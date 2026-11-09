//! Pure parsing and validation helpers (no I/O).

use macrunnerbar_core::ApiError;
use serde_json::Value;

use crate::RUNNER_PREFIX;

fn simple_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Reject anything but `owner/name` made of GitHub-legal characters, so a repo string can never
/// alter the request path or query.
pub(crate) fn check_repo(repo: &str) -> Result<(), ApiError> {
    match repo.split_once('/') {
        Some((o, n)) if simple_name(o) && simple_name(n) => Ok(()),
        _ => Err(ApiError::Invalid("repo must be owner/name".into())),
    }
}

/// Runner names we may create or delete: `ghm-` prefix, safe characters.
pub(crate) fn check_runner_name(name: &str) -> Result<(), ApiError> {
    if name.starts_with(RUNNER_PREFIX) && name.len() > RUNNER_PREFIX.len() && simple_name(name) {
        Ok(())
    } else {
        Err(ApiError::Invalid("runner name must start with ghm-".into()))
    }
}

pub(crate) fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::Invalid(format!("missing string field `{key}`")))
}

pub(crate) fn u64_field(v: &Value, key: &str) -> Result<u64, ApiError> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ApiError::Invalid(format!("missing numeric field `{key}`")))
}

pub(crate) fn bool_field(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Lowercase, trim, drop empties and duplicates, keep order.
pub(crate) fn normalize_labels(labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in labels {
        let l = l.trim().to_ascii_lowercase();
        if !l.is_empty() && !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

const SHA_BEGIN: &str = "<!-- BEGIN SHA osx-arm64 -->";
const SHA_END: &str = "<!-- END SHA osx-arm64 -->";

/// The osx-arm64 SHA-256 from the raw release body:
/// `<!-- BEGIN SHA osx-arm64 -->hex<!-- END SHA osx-arm64 -->`.
pub(crate) fn release_sha(body: &str) -> Result<String, ApiError> {
    let start = body.find(SHA_BEGIN).map(|i| i + SHA_BEGIN.len());
    let end = body.find(SHA_END);
    let sha = match (start, end) {
        (Some(s), Some(e)) if s <= e => body[s..e].trim().to_ascii_lowercase(),
        _ => {
            return Err(ApiError::Invalid(
                "release has no osx-arm64 checksum".into(),
            ));
        }
    };
    if sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(sha)
    } else {
        Err(ApiError::Invalid(
            "osx-arm64 checksum is not SHA-256 hex".into(),
        ))
    }
}

/// A runner version such as `2.338.2` (digits, dots, optional `-suffix`), safe to put in a file name.
pub(crate) fn check_version(v: &str) -> Result<(), ApiError> {
    let ok = !v.is_empty()
        && v.chars().next().is_some_and(|c| c.is_ascii_digit())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::Invalid("unexpected runner version".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_validation() {
        assert!(check_repo("owner/repo").is_ok());
        assert!(check_repo("My-Org/some.repo_1").is_ok());
        for bad in [
            "", "owner", "a/b/c", "a/b?x=1", "a/../b", "../b", "a/..", "a/b c", "/b", "a/",
            "a/b#x", "a/b%2f",
        ] {
            assert!(check_repo(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn runner_name_validation() {
        assert!(check_runner_name("ghm-owner-repo-abc123").is_ok());
        for bad in ["gha-x", "ghw-x", "ghm-", "ghm-a/b", "ghm-a b", "GHM-x", ""] {
            assert!(check_runner_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn labels_are_lowercased_and_deduped() {
        let l = [
            "Self-Hosted".to_string(),
            "macOS".into(),
            " ARM64 ".into(),
            "macos".into(),
            "".into(),
        ];
        assert_eq!(normalize_labels(&l), vec!["self-hosted", "macos", "arm64"]);
    }

    #[test]
    fn release_sha_from_real_body_shape() {
        let sha = "25a065af409ffa317ab4d5a3b614dd15fc9c40f2172bc1b35a1b1258e919a388";
        let body = format!(
            "- actions-runner-win-x64-2.338.2.zip <!-- BEGIN SHA win-x64 -->{}<!-- END SHA win-x64 -->\n- actions-runner-osx-arm64-2.338.2.tar.gz <!-- BEGIN SHA osx-arm64 -->{sha}<!-- END SHA osx-arm64 -->\n",
            "f".repeat(64)
        );
        assert_eq!(release_sha(&body).as_deref(), Ok(sha));
    }

    #[test]
    fn release_sha_rejects_missing_or_malformed() {
        assert!(release_sha("nothing here").is_err());
        assert!(release_sha("<!-- BEGIN SHA osx-arm64 -->abc<!-- END SHA osx-arm64 -->").is_err());
        assert!(
            release_sha(&format!(
                "<!-- BEGIN SHA osx-arm64 -->{}<!-- END SHA osx-arm64 -->",
                "z".repeat(64)
            ))
            .is_err()
        );
        assert!(release_sha("<!-- END SHA osx-arm64 -->x<!-- BEGIN SHA osx-arm64 -->").is_err());
    }

    #[test]
    fn version_validation() {
        assert!(check_version("2.338.2").is_ok());
        assert!(check_version("2.338.2-rc.1").is_ok());
        for bad in ["", "v2.1", "../x", "2.1/../x", "2 1"] {
            assert!(check_version(bad).is_err(), "{bad}");
        }
    }
}
