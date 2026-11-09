//! The event loop: the tray item, the popover window and the Preferences window, driving the engine.

use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lune_ui::input::control_flow;
use lune_ui::material::{self, Material};
use lune_ui::tray::{TrayAction, TrayEvent, TrayRect, set_event_handler};
use lune_ui::window::glass_attributes;
use macrunnerbar_core::engine::{EngineHandle, Observer};
use macrunnerbar_core::{Clock, Settings, StatusSnapshot};
use macrunnerbar_macos::SystemClock;
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::host::{PopoverHost, TrayHost};
use crate::popover_ui::{self, PopoverAction};
use crate::prefs::PrefsForm;
use crate::prefs_ui::{self, PrefsAction};
use crate::render::{Renderer, Target};
use crate::view::{popover_view, safe_url};
use crate::wiring::Services;

/// What the event loop is woken with.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AppEvent {
    /// The tray item was clicked.
    Tray(TrayEvent),
    /// The engine's snapshot changed.
    Snapshot,
}

/// How the app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Normal: the engine runs.
    Run,
    /// A two second check that the tray, the GPU and a popover frame work; no engine, no network, no
    /// Keychain.
    Smoke,
}

/// When the smoke opens the popover, and when it ends.
const SMOKE_OPEN: Duration = Duration::from_millis(800);
const SMOKE_END: Duration = Duration::from_millis(2000);

struct PrefsWindow {
    window: Arc<Window>,
    target: Target,
    form: PrefsForm,
    base: Settings,
    visible: bool,
}

pub(crate) struct App {
    mode: Mode,
    services: Option<Services>,
    engine: Option<EngineHandle>,
    renderer: Option<Renderer>,
    popover: Option<PopoverHost>,
    tray: Option<TrayHost>,
    prefs: Option<PrefsWindow>,
    clock: SystemClock,
    start: Instant,
    pop_flow: ControlFlow,
    prefs_flow: ControlFlow,
    smoke_opened: bool,
    smoke_frames: u32,
    exit_code: u8,
}

/// An event loop for the app. On macOS it keeps the app from taking the front when the popover
/// shows (LuneUI's rule for a popover that must not steal focus).
fn event_loop() -> Result<EventLoop<AppEvent>, String> {
    let mut builder = EventLoop::<AppEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_activate_ignoring_other_apps(false);
        // No Dock icon, also while running unbundled (the bundle's `LSUIElement` does the same).
        use winit::platform::macos::ActivationPolicy;
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    builder.build().map_err(|e| format!("event loop: {e}"))
}

/// Opens a run in the browser. Only github.com URLs are passed on, and as an argument, never to a shell.
fn open_in_browser(url: &str) {
    let Some(url) = safe_url(url) else { return };
    let _ = Command::new("/usr/bin/open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

impl App {
    fn new(mode: Mode, services: Option<Services>, engine: Option<EngineHandle>) -> Self {
        Self {
            mode,
            services,
            engine,
            renderer: None,
            popover: None,
            tray: None,
            prefs: None,
            clock: SystemClock,
            start: Instant::now(),
            pop_flow: ControlFlow::Wait,
            prefs_flow: ControlFlow::Wait,
            smoke_opened: false,
            smoke_frames: 0,
            exit_code: 0,
        }
    }

    fn snapshot(&self) -> StatusSnapshot {
        match &self.engine {
            Some(e) => e.snapshot(),
            None => smoke_snapshot(self.clock.now()),
        }
    }

    fn set_flow(&self, event_loop: &ActiveEventLoop) {
        let mut flow = merge_flow(self.pop_flow, self.prefs_flow);
        if self.tray.as_ref().is_some_and(TrayHost::wants_poll) {
            flow = merge_flow(
                flow,
                ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50)),
            );
        }
        if self.mode == Mode::Smoke {
            let target = if self.smoke_opened {
                SMOKE_END
            } else {
                SMOKE_OPEN
            };
            flow = merge_flow(flow, ControlFlow::WaitUntil(self.start + target));
        }
        event_loop.set_control_flow(flow);
    }

    /// Stops the engine (killing every runner) and leaves.
    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(p) = &mut self.popover {
            p.close();
        }
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
        self.tray = None;
        event_loop.exit();
    }

    fn open_prefs(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(services), Some(r)) = (&self.services, &mut self.renderer) else {
            return;
        };
        let base = services.load_settings();
        let form =
            PrefsForm::from_settings(&base, services.token_stored(), services.login_enabled());
        if self.prefs.is_none() {
            let (w, h) = prefs_ui::SIZE;
            let attrs = glass_attributes(
                "MacRunnerbar Preferences",
                (f64::from(w), f64::from(h)),
                (f64::from(w), f64::from(h)),
                false,
            )
            .with_resizable(false);
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    eprintln!("preferences window: {e}");
                    return;
                }
            };
            if let Err(e) = material::apply(&window, Material::Vibrancy) {
                eprintln!("preferences glass: {e}");
            }
            match Target::new(&mut r.gpu, &window) {
                Ok(target) => {
                    self.prefs = Some(PrefsWindow {
                        window,
                        target,
                        form: form.clone(),
                        base: base.clone(),
                        visible: false,
                    });
                }
                Err(e) => {
                    eprintln!("preferences surface: {e}");
                    return;
                }
            }
        }
        if let Some(p) = &mut self.prefs {
            p.form = form;
            p.base = base;
            p.visible = true;
            p.window.set_visible(true);
            p.window.focus_window();
            p.window.request_redraw();
        }
    }

    fn handle_popover_action(&mut self, event_loop: &ActiveEventLoop, action: PopoverAction) {
        match action {
            PopoverAction::Open(url) => {
                open_in_browser(&url);
                self.close_popover();
            }
            PopoverAction::TogglePause => {
                if let Some(e) = &self.engine {
                    e.set_paused(!e.snapshot().paused);
                }
            }
            PopoverAction::Preferences => {
                self.close_popover();
                self.open_prefs(event_loop);
            }
            PopoverAction::Quit => self.quit(event_loop),
        }
    }

    fn close_popover(&mut self) {
        if let Some(p) = &mut self.popover {
            p.close();
        }
        if let Some(t) = &mut self.tray {
            t.closed();
        }
    }

    fn handle_prefs_action(&mut self, action: PrefsAction) {
        let (Some(services), Some(prefs)) = (&self.services, &mut self.prefs) else {
            return;
        };
        match action {
            PrefsAction::SaveToken(token) => {
                prefs.form.message = Some(match services.save_token(token) {
                    Ok(()) => {
                        prefs.form.token_stored = true;
                        (true, "Token saved to the Keychain.".to_string())
                    }
                    Err(e) => (false, e),
                });
            }
            PrefsAction::RemoveToken => {
                prefs.form.message = Some(match services.remove_token() {
                    Ok(()) => {
                        prefs.form.token_stored = false;
                        (true, "Token removed.".to_string())
                    }
                    Err(e) => (false, e),
                });
            }
            PrefsAction::Save(settings) => {
                prefs.form.message = Some(match services.save_settings(&settings) {
                    Ok(()) => {
                        prefs.base = settings;
                        (true, "Saved.".to_string())
                    }
                    Err(e) => (false, e),
                });
            }
            PrefsAction::Close => {
                prefs.visible = false;
                prefs.window.set_visible(false);
                return;
            }
        }
        // New settings or a new token: look at GitHub again now.
        if let Some(e) = &self.engine {
            e.scan_now();
        }
        prefs.window.request_redraw();
    }

    fn redraw_popover(&mut self, event_loop: &ActiveEventLoop) {
        let now = self.start.elapsed();
        let snap = self.snapshot();
        let view = popover_view(&snap, self.clock.now());
        let (Some(pop), Some(r)) = (&mut self.popover, &mut self.renderer) else {
            return;
        };
        match pop.render(r, &view, now) {
            Ok(Some(frame)) => {
                self.smoke_frames += 1;
                let mut flow = if frame.hidden {
                    ControlFlow::Wait
                } else {
                    control_flow(frame.drawn.repaint, self.start).0
                };
                // Elapsed times tick: redraw each second while the popover is up and something runs.
                if pop.is_visible() && !view.jobs.is_empty() {
                    flow = merge_flow(
                        flow,
                        ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1)),
                    );
                }
                if !frame.hidden && control_flow(frame.drawn.repaint, self.start).1 {
                    pop.window().request_redraw();
                }
                self.pop_flow = flow;
                if frame.hidden
                    && let Some(t) = &mut self.tray
                {
                    t.closed();
                }
                if let Some(action) = frame.action {
                    self.handle_popover_action(event_loop, action);
                }
            }
            Ok(None) => self.pop_flow = ControlFlow::Wait,
            Err(e) => {
                eprintln!("popover: {e}");
                self.exit_code = 1;
                event_loop.exit();
            }
        }
        self.set_flow(event_loop);
    }

    fn redraw_prefs(&mut self, event_loop: &ActiveEventLoop) {
        let now = self.start.elapsed();
        let (Some(prefs), Some(r)) = (&mut self.prefs, &mut self.renderer) else {
            return;
        };
        if !prefs.visible {
            return;
        }
        let mut action = None;
        let window = prefs.window.clone();
        let (form, base) = (&mut prefs.form, &prefs.base);
        match prefs.target.render(r, &window, now, |ui| {
            let area = ui.painter().window;
            action = prefs_ui::draw(ui, area, form, base);
        }) {
            Ok(Some(drawn)) => {
                let (flow, redraw) = control_flow(drawn.repaint, self.start);
                self.prefs_flow = flow;
                if redraw {
                    window.request_redraw();
                }
            }
            Ok(None) => self.prefs_flow = ControlFlow::Wait,
            Err(e) => {
                eprintln!("preferences: {e}");
                self.exit_code = 1;
                event_loop.exit();
            }
        }
        if let Some(a) = action {
            self.handle_prefs_action(a);
        }
        self.set_flow(event_loop);
    }

    fn smoke_step(&mut self, event_loop: &ActiveEventLoop) {
        let elapsed = self.start.elapsed();
        if !self.smoke_opened && elapsed >= SMOKE_OPEN {
            self.smoke_opened = true;
            let rect = self
                .tray
                .as_ref()
                .and_then(TrayHost::rect)
                .unwrap_or(TrayRect {
                    x: 1000.0,
                    y: 0.0,
                    w: 30.0,
                    h: 24.0,
                });
            let view = popover_view(&self.snapshot(), self.clock.now());
            if let Some(p) = &mut self.popover {
                p.open(
                    lune_ui::tray::TraySurface::MenuUnderBar,
                    rect,
                    (popover_ui::WIDTH, popover_ui::height(&view)),
                );
            }
        }
        if elapsed >= SMOKE_END {
            if self.smoke_frames == 0 {
                eprintln!("smoke: the popover drew no frame");
                self.exit_code = 1;
            }
            self.tray = None;
            event_loop.exit();
        }
    }
}

/// A fixed snapshot for the smoke: one running job.
fn smoke_snapshot(now: macrunnerbar_core::Timestamp) -> StatusSnapshot {
    let mut s = StatusSnapshot::idle(2, now);
    s.running.push(macrunnerbar_core::RunningJob {
        runner_name: "ghm-smoke-000000".into(),
        repo: "owner/repo".into(),
        workflow: Some("CI".into()),
        job_name: Some("build".into()),
        html_url: Some("https://github.com/owner/repo/actions/runs/1".into()),
        started: now,
    });
    s
}

/// The earlier of two control flows: polling beats a deadline, a deadline beats waiting.
fn merge_flow(a: ControlFlow, b: ControlFlow) -> ControlFlow {
    match (a, b) {
        (ControlFlow::Poll, _) | (_, ControlFlow::Poll) => ControlFlow::Poll,
        (ControlFlow::WaitUntil(x), ControlFlow::WaitUntil(y)) => ControlFlow::WaitUntil(x.min(y)),
        (ControlFlow::WaitUntil(t), ControlFlow::Wait)
        | (ControlFlow::Wait, ControlFlow::WaitUntil(t)) => ControlFlow::WaitUntil(t),
        (ControlFlow::Wait, ControlFlow::Wait) => ControlFlow::Wait,
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.popover.is_some() {
            return;
        }
        let (popover, renderer) = match PopoverHost::create(event_loop) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        let scale = popover.window().scale_factor();
        self.popover = Some(popover);
        self.renderer = Some(renderer);
        match TrayHost::new(&self.snapshot(), scale) {
            Ok(t) => self.tray = Some(t),
            Err(e) => {
                eprintln!("tray: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        }
        self.set_flow(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Snapshot => {
                let snap = self.snapshot();
                if let Some(t) = &mut self.tray {
                    t.sync(event_loop, &snap);
                }
                if let Some(p) = &self.popover
                    && p.is_visible()
                {
                    p.window().request_redraw();
                }
            }
            AppEvent::Tray(click) => {
                let now = self.start.elapsed();
                let Some((action, rect)) = self.tray.as_mut().and_then(|t| t.click(click, now))
                else {
                    return;
                };
                match action {
                    TrayAction::Open(kind) => {
                        let view = popover_view(&self.snapshot(), self.clock.now());
                        if let Some(p) = &mut self.popover {
                            p.open(kind, rect, (popover_ui::WIDTH, popover_ui::height(&view)));
                        }
                    }
                    TrayAction::Close => self.close_popover(),
                    TrayAction::None => {}
                }
            }
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if !matches!(cause, StartCause::ResumeTimeReached { .. }) {
            return;
        }
        let snap = self.snapshot();
        if let Some(t) = &mut self.tray {
            t.sync(event_loop, &snap);
        }
        if let Some(p) = &self.popover
            && p.is_visible()
        {
            p.window().request_redraw();
        }
        if let Some(p) = &self.prefs
            && p.visible
        {
            p.window.request_redraw();
        }
        if self.mode == Mode::Smoke {
            self.smoke_step(event_loop);
        }
        self.set_flow(event_loop);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
        self.tray = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if event_loop.exiting() {
            return;
        }
        if self.popover.as_ref().map(PopoverHost::id) == Some(id) {
            if matches!(event, WindowEvent::RedrawRequested) {
                self.redraw_popover(event_loop);
            } else {
                let now = self.start.elapsed();
                let lost = self
                    .popover
                    .as_mut()
                    .is_some_and(|p| p.window_event(&event));
                if lost {
                    if let Some(t) = &mut self.tray {
                        t.focus_lost(now);
                    }
                    if let Some(p) = &mut self.popover {
                        p.close();
                    }
                }
            }
            return;
        }
        if self.prefs.as_ref().map(|p| p.window.id()) == Some(id) {
            match &event {
                WindowEvent::CloseRequested => {
                    if let Some(p) = &mut self.prefs {
                        p.visible = false;
                        p.window.set_visible(false);
                    }
                }
                WindowEvent::RedrawRequested => self.redraw_prefs(event_loop),
                other => {
                    if let (Some(p), Some(r)) = (&mut self.prefs, &self.renderer) {
                        if let WindowEvent::Resized(size) = other {
                            p.target.surface.resize(&r.gpu, size.width, size.height);
                        }
                        p.target.translator.window_event(other);
                        p.window.request_redraw();
                    }
                }
            }
        }
    }
}

/// Runs the app until it quits. `Mode::Smoke` exits after about two seconds.
pub(crate) fn run(mode: Mode) -> ExitCode {
    let event_loop = match event_loop() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let proxy: EventLoopProxy<AppEvent> = event_loop.create_proxy();
    let tray_proxy = proxy.clone();
    set_event_handler(move |event| {
        let _ = tray_proxy.send_event(AppEvent::Tray(event));
    });
    let (services, engine) = if mode == Mode::Run {
        let snapshot_proxy = proxy.clone();
        let observer: Observer = Arc::new(move |_| {
            let _ = snapshot_proxy.send_event(AppEvent::Snapshot);
        });
        match Services::start(observer) {
            Ok((s, e)) => (Some(s), Some(e)),
            Err(crate::wiring::StartError::AlreadyRunning) => {
                eprintln!("MacRunnerbar is already running.");
                return ExitCode::SUCCESS;
            }
            Err(crate::wiring::StartError::Failed(e)) => {
                eprintln!("cannot start: {e}");
                return ExitCode::from(1);
            }
        }
    } else {
        (None, None)
    };
    let mut app = App::new(mode, services, engine);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop: {e}");
        return ExitCode::from(1);
    }
    ExitCode::from(app.exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_earlier_flow_wins() {
        let (soon, later) = (Instant::now(), Instant::now() + Duration::from_secs(1));
        let until = ControlFlow::WaitUntil;
        assert_eq!(
            merge_flow(ControlFlow::Wait, ControlFlow::Wait),
            ControlFlow::Wait
        );
        assert_eq!(merge_flow(until(later), ControlFlow::Wait), until(later));
        assert_eq!(merge_flow(until(later), until(soon)), until(soon));
        assert_eq!(
            merge_flow(until(soon), ControlFlow::Poll),
            ControlFlow::Poll
        );
    }

    #[test]
    fn the_browser_only_gets_github_urls() {
        // `open_in_browser` ignores anything `safe_url` refuses; a refused URL starts no process.
        assert!(safe_url("file:///Applications/Calculator.app").is_none());
        assert!(safe_url("https://github.com/o/r/actions/runs/1").is_some());
    }

    #[test]
    fn smoke_snapshot_has_one_running_job() {
        let s = smoke_snapshot(macrunnerbar_core::Timestamp::from_millis(5));
        assert_eq!(s.running_count(), 1);
    }
}
