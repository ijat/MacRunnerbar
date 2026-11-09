//! `--dump <dir>`: renders the popover, Preferences and the icons offscreen to PNG files, so the look
//! can be checked without a display. Headless GPU, no window, no tray.

use std::path::Path;
use std::time::Duration;

use lune_ui::dump;
use lune_ui::frame::{FrameParams, FrameRenderer};
use lune_ui::gpu::{self, Gpu};
use lune_ui::input::keymap;
use lune_ui::lunegit_dark::THEME;
use lune_ui::motion::MotionTier;
use lune_ui::shortcuts::Shortcuts;
use lune_ui::strings::{DisplayFormat, StringTable};
use lune_ui::testing::ScriptedInput;
use lune_ui::ui::{FrameConfig, Ui, UiStore};
use macrunnerbar_core::{IconState, Settings};

use crate::icon::{self, Tone};
use crate::popover_ui;
use crate::prefs::PrefsForm;
use crate::prefs_ui;
use crate::render::{Renderer, text_cache};
use crate::view::{JobRow, Mood, PopoverView};

const SCALE: f32 = 2.0;

fn sample_view(jobs: usize) -> PopoverView {
    PopoverView {
        title: "MacRunnerbar".into(),
        state_text: if jobs > 0 {
            format!("{jobs} running")
        } else {
            "Idle".into()
        },
        mood: if jobs > 0 { Mood::Busy } else { Mood::Ok },
        runners: format!("{jobs}/2"),
        paused: false,
        jobs: (0..jobs)
            .map(|i| JobRow {
                repo: ["ijat/MacRunnerbar", "example-org/example-app"][i % 2].into(),
                detail: ["CI / build (macos-arm64)", "Release / package"][i % 2].into(),
                elapsed: ["3m 12s", "41s"][i % 2].into(),
                url: Some("https://github.com/ijat/MacRunnerbar/actions/runs/1".into()),
            })
            .collect(),
        more_jobs: 0,
        waiting: if jobs > 1 {
            vec!["1 job waiting on the runner cap: ijat/other".into()]
        } else {
            Vec::new()
        },
        more_waiting: 0,
        last_error: (jobs == 0)
            .then(|| "ijat/broken: polling failed: HTTP 500; retrying in 60s".to_string()),
    }
}

/// Draws one frame of `draw` at 2x into a PNG.
fn scene(
    r: &mut Renderer,
    format: wgpu::TextureFormat,
    frame: &mut FrameRenderer,
    path: &Path,
    size: (f32, f32),
    draw: impl FnOnce(&mut Ui<'_>),
) -> Result<(), String> {
    let px = ((size.0 * SCALE) as u32, (size.1 * SCALE) as u32);
    let texture = dump::target(&r.gpu.device, format, px);
    let view = texture.create_view(&Default::default());
    let mut store = UiStore::default();
    let strings = StringTable::default();
    let shortcuts = Shortcuts::default();
    let mut input = ScriptedInput::new(size, SCALE);
    input.set_time(Duration::from_millis(1000));
    let raw = input.end_frame();
    let config = FrameConfig {
        theme: THEME,
        tier: MotionTier::Full,
        strings: &strings,
        arg_format: &DisplayFormat,
        keymap: keymap(),
        shortcuts: &shortcuts,
    };
    let mut ui = Ui::begin(&mut store, &raw, config, &mut r.text);
    draw(&mut ui);
    let (display, _) = ui.finish();
    let params = FrameParams {
        size: px,
        scale: SCALE,
        blur_sigma: 14.0,
        saturation: 1.4,
        ..FrameParams::default()
    };
    for _ in 0..2 {
        frame.draw_with(&mut r.gpu, &mut r.text, &display, &view, params, &mut [])?;
        r.gpu.end_frame(&mut r.text);
    }
    let rgba = dump::read_rgba(&r.gpu.device, &r.gpu.queue, &texture)?;
    dump::write_png(path, px, &rgba)?;
    eprintln!("dumped {}", path.display());
    Ok(())
}

pub(crate) fn run(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let gpu = pollster::block_on(Gpu::new(gpu::instance(None), None))?;
    let mut r = Renderer {
        gpu,
        text: text_cache(),
    };
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut frame = FrameRenderer::new(&mut r.gpu, format);

    for (name, jobs) in [("popover-idle", 0), ("popover-running", 2)] {
        let view = sample_view(jobs);
        let size = (popover_ui::WIDTH, popover_ui::height(&view));
        scene(
            &mut r,
            format,
            &mut frame,
            &dir.join(format!("{name}.png")),
            size,
            |ui| {
                // A flat stand-in for the OS glass behind the window.
                let area = ui.painter().window;
                ui.painter().fill(area, 0.0, [0x3A, 0x3C, 0x44, 0xFF]);
                popover_ui::draw(ui, area, &view);
            },
        )?;
    }

    let mut form = PrefsForm::from_settings(&Settings::default(), true, true);
    form.message = Some((true, "Saved.".into()));
    scene(
        &mut r,
        format,
        &mut frame,
        &dir.join("preferences.png"),
        prefs_ui::SIZE,
        |ui| {
            let area = ui.painter().window;
            ui.painter().fill(area, 0.0, [0x3A, 0x3C, 0x44, 0xFF]);
            prefs_ui::draw(ui, area, &mut form, &Settings::default());
        },
    )?;

    // The icons: every state at 2x on a dark and a light bar, enlarged 6x (nearest neighbour).
    let mut sheet = vec![0u8; 0];
    let (cell, factor) = (36 * 6, 6usize);
    let states = [
        (IconState::Idle, 0),
        (IconState::Running, 3),
        (IconState::Running, 12),
        (IconState::Error, 0),
        (IconState::Paused, 0),
    ];
    let (w, h) = (cell * states.len(), cell * 2);
    sheet.resize(w * h * 4, 0);
    for (row, (tone, bar)) in [
        (Tone::Light, [0x25u8, 0x25, 0x28, 0xFF]),
        (Tone::Dark, [0xE6u8, 0xE6, 0xE9, 0xFF]),
    ]
    .into_iter()
    .enumerate()
    {
        for (col, (state, count)) in states.iter().enumerate() {
            let img = icon::render(*state, *count, tone, 2);
            for y in 0..cell {
                for x in 0..cell {
                    let src = ((y / factor) * 36 + x / factor) * 4;
                    let a = u32::from(img.rgba[src + 3]);
                    let dst = ((row * cell + y) * w + col * cell + x) * 4;
                    for c in 0..3 {
                        let v = (u32::from(img.rgba[src + c]) * a + u32::from(bar[c]) * (255 - a))
                            / 255;
                        sheet[dst + c] = v as u8;
                    }
                    sheet[dst + 3] = 255;
                }
            }
        }
    }
    dump::write_png(&dir.join("icons.png"), (w as u32, h as u32), &sheet)?;
    eprintln!("dumped {}", dir.join("icons.png").display());
    Ok(())
}
