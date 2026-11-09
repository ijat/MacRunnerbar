//! MacRunnerbar: a menu-bar app that runs per-job JIT GitHub Actions runners on this Mac.
//!
//! This crate is the composition root (`wiring`) and the only one that depends on LuneUI. Everything
//! else is behind the traits of `macrunnerbar-core`.
//!
//! - no arguments: run the app (tray item, popover, Preferences);
//! - `--smoke`: put a tray item up for about two seconds, draw one popover frame and exit. No engine,
//!   no network, no Keychain. Exit 0 when it held, 1 when a check failed, 2 when there is no GUI session;
//! - `--dump <dir>`: render the popover, Preferences and icons offscreen to PNG files.

mod app;
mod dump;
mod host;
mod icon;
mod popover_ui;
mod prefs;
mod prefs_ui;
mod render;
mod view;
mod wiring;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: macrunnerbar [--smoke | --dump <dir>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => app::run(app::Mode::Run),
        [a] if a == "--smoke" => app::run(app::Mode::Smoke),
        [a, dir] if a == "--dump" => match dump::run(&PathBuf::from(dir)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("dump: {e}");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
