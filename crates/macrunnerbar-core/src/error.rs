//! Error types shared by the traits.

use std::fmt;
use std::time::Duration;

/// A failure from the GitHub API adapter. Messages never contain tokens or job payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 401: the token is missing, revoked or expired.
    Unauthorized,
    /// 403 that is not a rate limit, or a token without the needed scope.
    Forbidden(String),
    /// 404: repo, run or runner does not exist, or Actions / repo-level runners are disabled.
    NotFound,
    /// Primary or secondary rate limit. `retry_after` when GitHub said how long.
    RateLimited { retry_after: Option<Duration> },
    /// Any other non-success status.
    Http { status: u16 },
    /// Transport failure (DNS, connect, TLS, timeout): the network is down or flaky.
    Network(String),
    /// The response could not be understood, or a checksum did not match.
    Invalid(String),
}

impl ApiError {
    /// True for failures that say nothing about the repo itself (network down, bad token).
    pub fn is_global(&self) -> bool {
        matches!(self, Self::Unauthorized | Self::Network(_))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("GitHub rejected the token (401)"),
            Self::Forbidden(m) => write!(f, "forbidden: {m}"),
            Self::NotFound => f.write_str("not found (404)"),
            Self::RateLimited {
                retry_after: Some(d),
            } => {
                write!(f, "rate limited, retry in {}s", d.as_secs())
            }
            Self::RateLimited { retry_after: None } => f.write_str("rate limited"),
            Self::Http { status } => write!(f, "HTTP {status}"),
            Self::Network(m) => write!(f, "network error: {m}"),
            Self::Invalid(m) => write!(f, "invalid response: {m}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// A failure from an OS-facing adapter (runner host, Keychain, settings, login item, notifier).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsError {
    /// What failed, without secrets.
    pub message: String,
}

impl OsError {
    /// Build from any message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OsError {}
