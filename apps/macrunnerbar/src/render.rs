//! One GPU and text cache for the app, and a per-window `Target`: surface, frame renderer and the UI
//! store, so the popover and the Preferences window share the device (LuneUI's rule: one `Gpu`, one
//! `TextCache`).

use std::sync::Arc;
use std::time::Duration;

use lune_ui::clipboard::Clipboard;
use lune_ui::frame::{FrameParams, FrameRenderer};
use lune_ui::gpu::Gpu;
use lune_ui::input::{ClipboardRequest, OutputState, Repaint, Translator, keymap};
use lune_ui::lunegit_dark::THEME;
use lune_ui::motion::MotionTier;
use lune_ui::shortcuts::Shortcuts;
use lune_ui::strings::{DisplayFormat, StringTable};
use lune_ui::surface::{Acquired, WindowSurface};
use lune_ui::text::TextCache;
use lune_ui::ui::{FrameConfig, Ui, UiStore};
use winit::window::Window;

/// The app's font: RznSans for everything (no separate monospace face is shipped).
const FONT: &str = "RznSans";

/// The shared GPU and text.
pub(crate) struct Renderer {
    pub(crate) gpu: Gpu,
    pub(crate) text: TextCache,
}

/// A text cache with the app's fonts.
pub(crate) fn text_cache() -> TextCache {
    TextCache::new(
        &[
            include_bytes!("../assets/RznSans-Regular.ttf").as_slice(),
            include_bytes!("../assets/RznSans-Medium.ttf"),
        ],
        FONT,
        FONT,
    )
}

/// Blur and saturation of the glass behind a surface (the toolkit's gallery values).
const BLUR_SIGMA: f32 = 14.0;
const BLUR_SATURATION: f32 = 1.4;

/// What drawing one frame of a window produced.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drawn {
    pub(crate) repaint: Repaint,
}

/// Everything one window needs besides the window itself.
pub(crate) struct Target {
    // The surface holds a handle to the window and drops first.
    pub(crate) surface: WindowSurface,
    frame: FrameRenderer,
    store: UiStore,
    pub(crate) translator: Translator,
    output: OutputState,
    strings: StringTable,
    shortcuts: Shortcuts,
    clipboard: Clipboard,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").finish_non_exhaustive()
    }
}

impl Target {
    /// A target for `window` on `gpu`.
    pub(crate) fn new(gpu: &mut Gpu, window: &Arc<Window>) -> Result<Self, String> {
        let raw = gpu
            .instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        Self::from_surface(gpu, window, raw)
    }

    /// A target over a surface already made (the first window's, which the GPU was created with).
    pub(crate) fn from_surface(
        gpu: &mut Gpu,
        window: &Arc<Window>,
        raw: wgpu::Surface<'static>,
    ) -> Result<Self, String> {
        let size = window.inner_size();
        let surface = WindowSurface::new(gpu, raw, (size.width, size.height))?;
        let frame = FrameRenderer::new(gpu, surface.format());
        Ok(Self {
            surface,
            frame,
            store: UiStore::default(),
            translator: Translator::new(size, window.scale_factor()),
            output: OutputState::new(),
            strings: StringTable::default(),
            shortcuts: Shortcuts::default(),
            clipboard: Clipboard::new(),
        })
    }

    /// Draws one frame with `draw` building the UI. `now` is the app's time.
    pub(crate) fn render(
        &mut self,
        r: &mut Renderer,
        window: &Window,
        now: Duration,
        draw: impl FnOnce(&mut Ui<'_>),
    ) -> Result<Option<Drawn>, String> {
        // A size asked for a moment ago has not always arrived as an event yet.
        let inner = window.inner_size();
        if (inner.width, inner.height) != self.surface.size() {
            self.surface.resize(&r.gpu, inner.width, inner.height);
            self.translator.resized(inner);
        }
        let frame = match self.surface.acquire(&r.gpu)? {
            Acquired::Frame(frame) => frame,
            Acquired::Skip => return Ok(None),
            Acquired::Redraw => {
                window.request_redraw();
                return Ok(None);
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let size = self.surface.size();
        let scale = window.scale_factor() as f32;
        let input = self.translator.take(now);
        let config = FrameConfig {
            theme: THEME,
            tier: MotionTier::Full,
            strings: &self.strings,
            arg_format: &DisplayFormat,
            keymap: keymap(),
            shortcuts: &self.shortcuts,
        };
        let mut ui = Ui::begin(&mut self.store, &input, config, &mut r.text);
        draw(&mut ui);
        let (display, out) = ui.finish();
        // The app never asks a window to close from inside a frame, so the answer is not needed.
        let _ = self.output.apply(window, &out, scale);
        self.clipboard.service(&out, &mut self.translator);
        if out
            .clipboard
            .iter()
            .any(|c| matches!(c, ClipboardRequest::Read))
        {
            window.request_redraw();
        }
        let params = FrameParams {
            size,
            scale,
            blur_sigma: BLUR_SIGMA,
            saturation: BLUR_SATURATION,
            ..FrameParams::default()
        };
        self.frame
            .draw_with(&mut r.gpu, &mut r.text, &display, &view, params, &mut [])?;
        window.pre_present_notify();
        self.surface.present(&r.gpu, frame);
        r.gpu.end_frame(&mut r.text);
        Ok(Some(Drawn {
            repaint: out.repaint,
        }))
    }
}
