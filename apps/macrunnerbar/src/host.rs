//! The tray item and the window of its surface. Patterned on LuneUI's gallery host (`popover.rs`),
//! which is the toolkit's worked example; the differences are the app's own icon image (state and
//! count) and the app's own content in the window.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lune_ui::gpu::Gpu;
use lune_ui::material::{self, Material};
use lune_ui::paint::Rect;
use lune_ui::popover::PopoverWindow;
use lune_ui::tray::{
    Tray, TrayAction, TrayButton, TrayError, TrayEvent, TrayImage, TrayPlatform, TrayRect,
    TraySurface, TrayToggle, bar_from_tray, place_surface, tray_surface,
};
use lune_ui::ui::Ui;
use lune_ui::window::popover_attributes;
use macrunnerbar_core::{IconState, StatusSnapshot};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::icon::{self, Tone};
use crate::popover_ui::{self, PopoverAction};
use crate::render::{Drawn, Renderer, Target};
use crate::view::{PopoverView, tooltip};

/// The corner radius of the OS glass behind the popover.
const GLASS_RADIUS: f64 = 9.0;
/// How long the status item's rect must hold still before it is taken as where the item is.
const SETTLE: Duration = Duration::from_millis(300);
/// Polls for the bar's appearance before the guessed tone is kept (50 ms apart: about 5 s).
const TONE_POLLS: u32 = 100;

const fn platform() -> TrayPlatform {
    if cfg!(target_os = "macos") {
        TrayPlatform::MacOs
    } else {
        TrayPlatform::Windows
    }
}

/// Where the window of `kind`, `logical` px across, goes, in physical desktop pixels. `place_surface`
/// works in logical pixels in one coordinate space; the tray rect and the monitor come physical, so only
/// the offsets from the monitor's origin are divided by its scale, then the result is multiplied back.
pub(crate) fn placed_position(
    kind: TraySurface,
    screen: Rect,
    scale: f64,
    tray: TrayRect,
    logical: (f32, f32),
) -> (f64, f64) {
    let (ox, oy) = (f64::from(screen.x), f64::from(screen.y));
    let screen_l = Rect::new(
        0.0,
        0.0,
        (f64::from(screen.w) / scale) as f32,
        (f64::from(screen.h) / scale) as f32,
    );
    let tray_l = Rect::new(
        ((tray.x - ox) / scale) as f32,
        ((tray.y - oy) / scale) as f32,
        (tray.w / scale) as f32,
        (tray.h / scale) as f32,
    );
    let placed = place_surface(kind, screen_l, bar_from_tray(screen_l, tray_l), logical);
    (
        ox + f64::from(placed.x) * scale,
        oy + f64::from(placed.y) * scale,
    )
}

/// What the popover frame reported.
#[derive(Debug)]
pub(crate) struct PopoverFrame {
    pub(crate) action: Option<PopoverAction>,
    pub(crate) drawn: Drawn,
    /// The window was hidden after this frame: closed and still.
    pub(crate) hidden: bool,
}

/// The window the popover fills: hidden until a tray click opens it.
pub(crate) struct PopoverHost {
    // The target (it holds a handle to the window) drops first, then the guard, which restores the
    // window, then the rest.
    pub(crate) target: Target,
    guard: PopoverWindow,
    kind: TraySurface,
    /// The tray rect the window was placed from, to place it again when its content changes size.
    anchor: Option<TrayRect>,
    size: (f32, f32),
    open: bool,
    visible: bool,
    /// The window gained focus since it opened: only then can a loss of it be real.
    had_focus: bool,
}

impl std::fmt::Debug for PopoverHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopoverHost")
            .field("open", &self.open)
            .field("visible", &self.visible)
            .finish_non_exhaustive()
    }
}

impl PopoverHost {
    /// Creates the hidden popover window and the GPU on its surface (the GPU must be made with a
    /// compatible surface, so the first window makes it).
    pub(crate) fn create(event_loop: &ActiveEventLoop) -> Result<(Self, Renderer), String> {
        let window = Arc::new(
            event_loop
                .create_window(popover_attributes((f64::from(popover_ui::WIDTH), 300.0)))
                .map_err(|e| format!("popover window: {e}"))?,
        );
        let guard = PopoverWindow::wrap(window.clone())?;
        if let Err(e) = material::apply_rounded(&window, Material::Vibrancy, GLASS_RADIUS) {
            eprintln!("popover glass: {e}");
        }
        let instance = lune_ui::gpu::instance(Some(Box::new(event_loop.owned_display_handle())));
        let raw = instance
            .create_surface(window.clone())
            .map_err(|e| format!("popover surface: {e}"))?;
        let mut gpu = pollster::block_on(Gpu::new(instance, Some(&raw)))?;
        let target = Target::from_surface(&mut gpu, &window, raw)?;
        let host = Self {
            target,
            guard,
            kind: TraySurface::MenuUnderBar,
            anchor: None,
            size: (popover_ui::WIDTH, 300.0),
            open: false,
            visible: false,
            had_focus: false,
        };
        Ok((
            host,
            Renderer {
                gpu,
                text: crate::render::text_cache(),
            },
        ))
    }

    pub(crate) fn window(&self) -> &Arc<Window> {
        self.guard.window()
    }

    pub(crate) fn id(&self) -> WindowId {
        self.window().id()
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    /// The monitor holding the tray rect's centre, else the primary.
    fn screen_of(&self, tray: TrayRect) -> (Rect, f64) {
        let window = self.window();
        let centre = (tray.x + tray.w / 2.0, tray.y + tray.h / 2.0);
        let rect_of = |m: &winit::monitor::MonitorHandle| {
            let (p, s) = (m.position(), m.size());
            (
                Rect::new(p.x as f32, p.y as f32, s.width as f32, s.height as f32),
                m.scale_factor(),
            )
        };
        window
            .available_monitors()
            .map(|m| rect_of(&m))
            .find(|(r, _)| r.contains((centre.0 as f32, centre.1 as f32)))
            .or_else(|| window.primary_monitor().map(|m| rect_of(&m)))
            .unwrap_or((Rect::new(0.0, 0.0, 1920.0, 1080.0), window.scale_factor()))
    }

    /// Opens (or moves and resizes) the window for `kind` under the tray item at `tray`.
    pub(crate) fn open(&mut self, kind: TraySurface, tray: TrayRect, logical: (f32, f32)) {
        let (screen, scale) = self.screen_of(tray);
        let at = placed_position(kind, screen, scale, tray, logical);
        let window = self.window().clone();
        // Size first, then move: macOS keeps a resized window's bottom edge where it was.
        let _ =
            window.request_inner_size(LogicalSize::new(f64::from(logical.0), f64::from(logical.1)));
        window.set_outer_position(PhysicalPosition::new(at.0.round(), at.1.round()));
        self.kind = kind;
        self.anchor = Some(tray);
        self.size = logical;
        self.open = true;
        self.had_focus = self.visible && window.has_focus();
        if !self.visible {
            self.guard.show();
            self.visible = true;
        }
        window.request_redraw();
    }

    /// Asks the content to go: the window hides once a frame finds it closed.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.window().request_redraw();
    }

    /// Feeds a window event; true when the window lost focus while open (the click rules need to know).
    pub(crate) fn window_event(&mut self, event: &WindowEvent) -> bool {
        if matches!(event, WindowEvent::Focused(true)) && self.open {
            self.had_focus = true;
        }
        let lost = matches!(event, WindowEvent::Focused(false))
            && self.open
            && self.visible
            && self.had_focus;
        if !matches!(
            event,
            WindowEvent::CloseRequested | WindowEvent::RedrawRequested
        ) {
            self.target.translator.window_event(event);
            self.window().request_redraw();
        }
        lost
    }

    /// Draws a frame if shown. The window is resized first when the content's height changed.
    pub(crate) fn render(
        &mut self,
        r: &mut Renderer,
        view: &PopoverView,
        now: Duration,
    ) -> Result<Option<PopoverFrame>, String> {
        if !self.visible {
            return Ok(None);
        }
        let wanted = (popover_ui::WIDTH, popover_ui::height(view));
        if self.open
            && (wanted.1 - self.size.1).abs() > 0.5
            && let Some(anchor) = self.anchor
        {
            self.open(self.kind, anchor, wanted);
        }
        let window = self.window().clone();
        let open = self.open;
        let mut action = None;
        let drawn = self.target.render(r, &window, now, |ui: &mut Ui<'_>| {
            if open {
                let area = ui.painter().window;
                action = popover_ui::draw(ui, area, view);
            }
        })?;
        let Some(drawn) = drawn else { return Ok(None) };
        let hidden = !self.open && drawn.repaint == lune_ui::input::Repaint::Idle;
        if hidden {
            self.guard.hide();
            self.visible = false;
        }
        Ok(Some(PopoverFrame {
            action,
            drawn,
            hidden,
        }))
    }
}

/// The item in the menu bar, its click rules and what it shows now.
#[derive(Debug)]
pub(crate) struct TrayHost {
    tray: Tray,
    toggle: TrayToggle,
    /// The picture and tooltip on the item: (state, count, tone).
    shown: (IconState, usize, Tone),
    tooltip: String,
    tone_known: bool,
    polls: u32,
    watch: Option<(TrayRect, Instant)>,
    settled: Option<TrayRect>,
    /// 1 or 2: which bitmap the item gets.
    scale: u32,
}

fn image(state: IconState, count: usize, tone: Tone, scale: u32) -> TrayImage {
    let img = icon::render(state, count, tone, scale);
    TrayImage {
        width: img.width,
        height: img.height,
        rgba: img.rgba,
    }
}

impl TrayHost {
    /// Puts the item in the tray. Call `lune_ui::tray::set_event_handler` first.
    pub(crate) fn new(snap: &StatusSnapshot, display_scale: f64) -> Result<Self, TrayError> {
        let scale = if display_scale >= 1.5 { 2 } else { 1 };
        // A guess until the OS has placed the item and can say what bar it is on.
        let tone = Tone::Light;
        let (state, count) = (snap.icon_state(), snap.running_count());
        let text = tooltip(snap);
        let tray = Tray::new(image(state, count, tone, scale), &text)?;
        Ok(Self {
            tray,
            toggle: TrayToggle::new(),
            shown: (state, count, tone),
            tooltip: text,
            tone_known: false,
            polls: 0,
            watch: None,
            settled: None,
            scale,
        })
    }

    /// The item's rect once it is on a monitor.
    fn placed_rect(&self, event_loop: &ActiveEventLoop) -> Option<TrayRect> {
        let rect = self.tray.rect()?;
        let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        event_loop
            .available_monitors()
            .any(|m| {
                let (p, s) = (m.position(), m.size());
                cx >= f64::from(p.x)
                    && cx < f64::from(p.x) + f64::from(s.width)
                    && cy >= f64::from(p.y)
                    && cy < f64::from(p.y) + f64::from(s.height)
            })
            .then_some(rect)
    }

    /// Watches the rect: settled once it has held still on a monitor for [`SETTLE`].
    fn observe(&mut self, event_loop: &ActiveEventLoop) {
        match (self.placed_rect(event_loop), self.watch) {
            (Some(now), Some((seen, since))) if now == seen => {
                if since.elapsed() >= SETTLE {
                    self.settled = Some(now);
                }
            }
            (Some(now), _) => {
                self.watch = Some((now, Instant::now()));
                self.settled = None;
            }
            (None, _) => {
                self.watch = None;
                self.settled = None;
            }
        }
    }

    /// Whether the item still waits to learn what bar it is on, so the loop should wake again soon.
    pub(crate) fn wants_poll(&self) -> bool {
        !self.tone_known && self.polls < TONE_POLLS
    }

    /// Puts the picture and tooltip right for `snap`, in the tone of the bar once it is known.
    pub(crate) fn sync(&mut self, event_loop: &ActiveEventLoop, snap: &StatusSnapshot) {
        let mut tone = self.shown.2;
        if self.wants_poll() {
            self.polls += 1;
            self.observe(event_loop);
            if let Some(dark) = self.settled.and_then(|_| self.tray.bar_is_dark()) {
                self.tone_known = true;
                tone = if dark { Tone::Light } else { Tone::Dark };
            }
        }
        let want = (snap.icon_state(), snap.running_count(), tone);
        if want != self.shown {
            if let Err(e) = self
                .tray
                .set_image(image(want.0, want.1, want.2, self.scale))
            {
                eprintln!("tray image: {e}");
            }
            self.shown = want;
        }
        let text = tooltip(snap);
        if text != self.tooltip {
            let _ = self.tray.set_tooltip(&text);
            self.tooltip = text;
        }
    }

    /// The item's rect, if the OS has placed it.
    pub(crate) fn rect(&self) -> Option<TrayRect> {
        self.tray.rect()
    }

    /// A click on the item: the toolkit's click rules say whether to open, close or ignore it.
    pub(crate) fn click(
        &mut self,
        event: TrayEvent,
        now: Duration,
    ) -> Option<(TrayAction, TrayRect)> {
        let TrayEvent::Click { button, rect } = event;
        let left = match button {
            TrayButton::Left => true,
            TrayButton::Right => false,
            TrayButton::Other => return None,
        };
        Some((self.toggle.click(tray_surface(platform(), left), now), rect))
    }

    /// The window lost focus while open: the click that took it must not reopen it.
    pub(crate) fn focus_lost(&mut self, now: Duration) {
        self.toggle.focus_lost(now);
    }

    /// The surface went away by itself (an action, Escape).
    pub(crate) fn closed(&mut self) {
        self.toggle.closed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tray(x: f64, y: f64, w: f64, h: f64) -> TrayRect {
        TrayRect { x, y, w, h }
    }

    #[test]
    fn the_menu_sits_4_logical_px_under_the_bar_at_scale_2() {
        // The numbers LuneUI measured on a Mac: a 4800 x 2700 monitor at scale 2, the item at
        // (3726, -2, 68, 66) physical; the menu is 4 logical px under the 32 px bar, flush right.
        let at = placed_position(
            TraySurface::MenuUnderBar,
            Rect::new(0.0, 0.0, 4800.0, 2700.0),
            2.0,
            tray(3726.0, -2.0, 68.0, 66.0),
            (220.0, 270.0),
        );
        assert_eq!(at, (4360.0, 72.0));
    }

    #[test]
    fn a_second_monitor_places_by_its_own_origin() {
        let first = placed_position(
            TraySurface::MenuUnderBar,
            Rect::new(0.0, 0.0, 4800.0, 2700.0),
            2.0,
            tray(3726.0, -2.0, 68.0, 66.0),
            (380.0, 300.0),
        );
        let second = placed_position(
            TraySurface::MenuUnderBar,
            Rect::new(4800.0, 0.0, 4800.0, 2700.0),
            2.0,
            tray(4800.0 + 3726.0, -2.0, 68.0, 66.0),
            (380.0, 300.0),
        );
        assert_eq!(second, (first.0 + 4800.0, first.1));
    }

    #[test]
    fn the_popover_stays_on_screen_when_the_item_is_near_the_right_edge() {
        let at = placed_position(
            TraySurface::MenuUnderBar,
            Rect::new(0.0, 0.0, 1512.0, 982.0),
            1.0,
            tray(1490.0, 0.0, 20.0, 24.0),
            (380.0, 400.0),
        );
        assert!(at.0 + 380.0 <= 1512.0 + 0.5, "right edge {}", at.0 + 380.0);
        assert!(at.0 >= 0.0 && at.1 >= 24.0);
    }
}
