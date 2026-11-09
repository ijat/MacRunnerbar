//! The Preferences form: plain data and validation, no UI types.
//!
//! The token field holds what the user is typing and nothing else: [`PrefsForm::take_token`] hands it
//! over as a `Secret` for the Keychain and clears the field. It is never part of `Settings` and never
//! formatted.

use macrunnerbar_core::{NotifyMode, Secret, Settings};

/// Bounds for the numeric settings.
pub(crate) const CAP_RANGE: (u32, u32) = (1, 32);
pub(crate) const POLL_RANGE: (u32, u32) = (5, 3600);

/// What the user is editing.
#[derive(Clone)]
pub(crate) struct PrefsForm {
    /// The token being typed (cleared once taken).
    pub(crate) token: String,
    /// The Keychain holds a token.
    pub(crate) token_stored: bool,
    pub(crate) labels: String,
    pub(crate) include: String,
    pub(crate) exclude: String,
    pub(crate) public_allow: String,
    pub(crate) per_owner: u32,
    pub(crate) global: u32,
    pub(crate) poll: u32,
    pub(crate) notify: NotifyMode,
    pub(crate) launch_at_login: bool,
    /// The outcome of the last action, for the status line: `(ok, text)`.
    pub(crate) message: Option<(bool, String)>,
}

// The token is deliberately left out of `Debug`.
impl std::fmt::Debug for PrefsForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrefsForm")
            .field("token", &"[redacted]")
            .field("token_stored", &self.token_stored)
            .field("global", &self.global)
            .finish_non_exhaustive()
    }
}

fn join(list: &[String]) -> String {
    list.join(", ")
}

/// Splits a list typed as comma, semicolon, space or newline separated entries.
pub(crate) fn split_list(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn valid_repo(s: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    s.split_once('/').is_some_and(|(o, n)| part(o) && part(n))
}

fn valid_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl PrefsForm {
    /// A form showing `s`.
    pub(crate) fn from_settings(s: &Settings, token_stored: bool, launch_at_login: bool) -> Self {
        Self {
            token: String::new(),
            token_stored,
            labels: join(&s.labels),
            include: join(&s.include),
            exclude: join(&s.exclude),
            public_allow: join(&s.public_allow),
            per_owner: u32::try_from(s.per_owner_max).unwrap_or(CAP_RANGE.1),
            global: u32::try_from(s.global_max).unwrap_or(CAP_RANGE.1),
            poll: u32::try_from(s.poll_interval_secs).unwrap_or(POLL_RANGE.1),
            notify: s.notify,
            launch_at_login,
            message: None,
        }
    }

    /// The settings the form describes, on top of `base` (fields the form does not show are kept).
    pub(crate) fn to_settings(&self, base: &Settings) -> Result<Settings, String> {
        let labels = split_list(&self.labels);
        if labels.is_empty() || !labels.iter().all(|l| valid_label(l)) {
            return Err("Labels: use letters, digits, - _ . separated by commas".into());
        }
        if !labels.iter().any(|l| l.eq_ignore_ascii_case("macos")) {
            return Err("Labels must include macos, or no job can match".into());
        }
        let repos = |name: &str, text: &str| -> Result<Vec<String>, String> {
            let list = split_list(text);
            match list.iter().find(|r| !valid_repo(r)) {
                Some(bad) => Err(format!("{name}: \"{bad}\" is not owner/name")),
                None => Ok(list),
            }
        };
        let in_range = |name: &str, v: u32, r: (u32, u32)| -> Result<u32, String> {
            if (r.0..=r.1).contains(&v) {
                Ok(v)
            } else {
                Err(format!("{name} must be {} to {}", r.0, r.1))
            }
        };
        Ok(Settings {
            poll_interval_secs: u64::from(in_range("Poll interval", self.poll, POLL_RANGE)?),
            per_owner_max: in_range("Per-owner cap", self.per_owner, CAP_RANGE)? as usize,
            global_max: in_range("Global cap", self.global, CAP_RANGE)? as usize,
            labels,
            include: repos("Include", &self.include)?,
            exclude: repos("Exclude", &self.exclude)?,
            public_allow: repos("Public allow list", &self.public_allow)?,
            notify: self.notify,
            launch_at_login: self.launch_at_login,
            job_user: base.job_user.clone(),
        })
    }

    /// The typed token as a `Secret`, clearing the field. `None` when nothing was typed.
    pub(crate) fn take_token(&mut self) -> Option<Secret<String>> {
        let typed = std::mem::take(&mut self.token);
        let trimmed = typed.trim();
        (!trimmed.is_empty()).then(|| Secret::new(trimmed.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> PrefsForm {
        PrefsForm::from_settings(&Settings::default(), false, false)
    }

    #[test]
    fn the_form_round_trips_the_settings() {
        let s = Settings {
            include: vec!["o/a".into(), "o/b".into()],
            exclude: vec!["o/c".into()],
            public_allow: vec!["o/pub".into()],
            per_owner_max: 3,
            global_max: 4,
            poll_interval_secs: 45,
            notify: NotifyMode::All,
            launch_at_login: true,
            job_user: Some("runner".into()),
            ..Settings::default()
        };
        let f = PrefsForm::from_settings(&s, true, true);
        assert_eq!(f.to_settings(&s).expect("valid"), s);
        assert!(f.token_stored);
    }

    #[test]
    fn lists_split_on_commas_semicolons_spaces_and_newlines() {
        assert_eq!(
            split_list("a/b, c/d;e/f\n g/h ,,"),
            vec!["a/b", "c/d", "e/f", "g/h"]
        );
        assert!(split_list(" \n ").is_empty());
    }

    #[test]
    fn bad_input_is_refused_with_a_message_naming_the_field() {
        let base = Settings::default();
        let mut f = form();
        f.include = "just-a-name".into();
        assert!(
            f.to_settings(&base)
                .expect_err("repo")
                .starts_with("Include:")
        );
        let mut f = form();
        f.exclude = "o/r?x=1".into();
        assert!(
            f.to_settings(&base)
                .expect_err("repo")
                .starts_with("Exclude:")
        );
        let mut f = form();
        f.public_allow = "../x".into();
        assert!(f.to_settings(&base).is_err());
        let mut f = form();
        f.labels = "self-hosted, arm64".into();
        assert!(f.to_settings(&base).expect_err("macos").contains("macos"));
        f.labels = "".into();
        assert!(f.to_settings(&base).is_err());
        f.labels = "macos, bad label!".into();
        assert!(f.to_settings(&base).is_err());
        let mut f = form();
        f.global = 0;
        assert!(
            f.to_settings(&base)
                .expect_err("cap")
                .contains("Global cap")
        );
        f.global = 2;
        f.per_owner = 33;
        assert!(
            f.to_settings(&base)
                .expect_err("cap")
                .contains("Per-owner cap")
        );
        f.per_owner = 2;
        f.poll = 4;
        assert!(
            f.to_settings(&base)
                .expect_err("poll")
                .contains("Poll interval")
        );
        f.poll = 30;
        assert!(f.to_settings(&base).is_ok());
    }

    #[test]
    fn the_token_leaves_the_form_only_as_a_secret_and_is_never_in_settings_or_debug() {
        let sentinel = concat!("gh", "p_SENTINELFORMTOKEN0123456789abcdef");
        let mut f = form();
        f.token = format!("  {sentinel}  ");
        let debug = format!("{f:?}");
        assert!(!debug.contains(sentinel), "Debug leaked the token");
        let settings = f.to_settings(&Settings::default()).expect("valid");
        assert!(!format!("{settings:?}").contains(sentinel));
        let secret = f.take_token().expect("a token was typed");
        assert_eq!(secret.expose(), sentinel, "trimmed, otherwise intact");
        assert!(f.token.is_empty(), "the field is cleared once taken");
        assert!(f.take_token().is_none());
        f.token = "   ".into();
        assert!(f.take_token().is_none(), "whitespace is not a token");
        assert!(!format!("{secret:?}{secret}").contains(sentinel));
    }
}
