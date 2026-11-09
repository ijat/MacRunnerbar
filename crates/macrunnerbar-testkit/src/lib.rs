//! In-memory fakes for every core trait, and a contract suite per trait that each real
//! implementation must also pass.
//!
//! Fakes are for tests of core and the engine: they record calls and let a test inject failures.
//! Contract functions panic (assert) on a violation, so an adapter's own test just calls them.

pub mod contract;
mod fake_github;
mod fake_host;
mod fake_os;

pub use fake_github::FakeGitHub;
pub use fake_host::FakeRunnerHost;
pub use fake_os::{FakeClock, FakeKeychain, FakeLoginItem, MemSettingsStore, RecordingNotifier};
