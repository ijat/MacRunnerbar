//! macOS adapters for MacRunnerbar. The only crate allowed to use `unsafe` (all of it in `sys`).
//!
//! - [`ProcessRunnerHost`]: per-job runner directories and process groups.
//! - [`SecurityCliKeychain`]: the token in the login Keychain via the `security` CLI.
//! - [`LaunchAgent`]: start at login through a LaunchAgent plist.
//! - [`logging`]: rotating file log with a redaction layer.
//! - [`OsascriptNotifier`], [`JsonSettingsStore`], [`SystemClock`].

mod extract;
mod host;
mod keychain;
pub mod logging;
mod login_item;
mod notifier;
mod paths;
mod settings;
mod sys;

pub use host::ProcessRunnerHost;
pub use keychain::SecurityCliKeychain;
pub use login_item::{LaunchAgent, LaunchAgentSpec};
pub use notifier::OsascriptNotifier;
pub use paths::Paths;
pub use settings::{JsonSettingsStore, SystemClock};
