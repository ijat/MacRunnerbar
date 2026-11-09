//! Runner registration names: `ghm-<repo slug>-<6 hex>`.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

/// Prefix of every runner this app registers. Cleanup touches nothing else.
pub const RUNNER_PREFIX: &str = "ghm-";

const MAX_SLUG: usize = 48;

/// Lowercase, every run of characters outside `[a-z0-9]` becomes one `-`, trimmed, at most 48 chars.
pub fn slug(repo: &str) -> String {
    let mut out = String::new();
    for c in repo.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    let cut: String = trimmed.chars().take(MAX_SLUG).collect();
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "repo".to_string()
    } else {
        cut.to_string()
    }
}

/// `ghm-<slug>-<suffix as 6 hex digits>`. Only `[a-z0-9-]` after the prefix, so every adapter accepts it.
pub fn runner_name(repo: &str, suffix: u32) -> String {
    format!("{RUNNER_PREFIX}{}-{:06x}", slug(repo), suffix & 0x00ff_ffff)
}

/// A per-process random suffix (std only: `RandomState` is seeded randomly). `counter` makes two calls
/// in one process differ even if the hasher state repeated.
pub fn random_suffix(counter: u64) -> u32 {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(counter);
    // Fold the 64-bit hash into 24 bits.
    let v = h.finish();
    u32::try_from((v ^ (v >> 24) ^ (v >> 48)) & 0x00ff_ffff).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_normalises_repo_names() {
        assert_eq!(slug("Example-Org/My_Repo.rs"), "example-org-my-repo-rs");
        assert_eq!(slug("//a//"), "a");
        assert_eq!(slug("日本/x"), "x");
        assert_eq!(slug("///"), "repo");
        let long = format!("owner/{}", "a".repeat(100));
        assert_eq!(slug(&long).len(), 48);
        assert!(!slug(&format!("{}/-x", "b".repeat(46))).ends_with('-'));
    }

    #[test]
    fn runner_names_have_prefix_slug_and_six_hex() {
        let n = runner_name("o/r", 0xabc);
        assert_eq!(n, "ghm-o-r-000abc");
        assert_eq!(runner_name("o/r", 0x0123_4567), "ghm-o-r-234567");
        assert!(
            n.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
    }

    #[test]
    fn random_suffix_varies_and_fits_24_bits() {
        let all: Vec<u32> = (0..64).map(random_suffix).collect();
        assert!(all.iter().all(|s| *s <= 0x00ff_ffff));
        let distinct: std::collections::HashSet<_> = all.iter().collect();
        assert!(distinct.len() > 50, "suffixes should be well spread");
    }
}
