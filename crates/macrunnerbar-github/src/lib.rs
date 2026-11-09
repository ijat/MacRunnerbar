//! GitHub REST adapter for MacRunnerbar: implements `macrunnerbar_core::GitHubApi` over `ureq`.
//!
//! - GET responses are cached per URL by `ETag`; a `304` replays the cached body and is not an error.
//! - The token is a `Secret`, is sent only to the configured API origin, and is scrubbed from every
//!   error string. Response bodies are never copied into errors (only GitHub's short `message` field,
//!   scrubbed, on a 403).
//! - Runners are only created, listed or deleted when their name starts with `ghm-`.

mod client;
mod http;
mod parse;

pub use client::GitHubClient;

/// Prefix of every runner registration this app creates; cleanup touches nothing else.
pub const RUNNER_PREFIX: &str = "ghm-";
