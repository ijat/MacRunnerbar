//! `Keychain` over the `security` CLI.
//!
//! The token never appears in an argument list (visible to every process in `ps`): it is written, hex
//! encoded, to the stdin of `security -i`, which reads its commands from stdin. Reading uses
//! `find-generic-password -w`, which prints the secret on stdout.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use macrunnerbar_core::{Keychain, OsError, Secret};

/// `security` exit status for "item not found".
const EXIT_NOT_FOUND: i32 = 44;
const DEFAULT_BIN: &str = "/usr/bin/security";
const MAX_TOKEN_LEN: usize = 1024;

/// The login-Keychain generic-password item that holds the token.
#[derive(Debug, Clone)]
pub struct SecurityCliKeychain {
    bin: PathBuf,
    service: String,
    account: String,
}

fn safe_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

impl SecurityCliKeychain {
    /// The item `service` / `account` (characters `[A-Za-z0-9._-]` only, so no quoting is ever needed).
    pub fn new(service: &str, account: &str) -> Result<Self, OsError> {
        if !safe_ident(service) || !safe_ident(account) {
            return Err(OsError::new(
                "keychain service and account must be [A-Za-z0-9._-]",
            ));
        }
        Ok(Self {
            bin: PathBuf::from(DEFAULT_BIN),
            service: service.into(),
            account: account.into(),
        })
    }

    /// The app's item: service `my.ijat.macrunnerbar`, account `github-pat`.
    pub fn standard() -> Self {
        Self {
            bin: PathBuf::from(DEFAULT_BIN),
            service: "my.ijat.macrunnerbar".into(),
            account: "github-pat".into(),
        }
    }

    /// Use another `security` binary (tests).
    #[must_use]
    pub fn with_binary(mut self, bin: impl Into<PathBuf>) -> Self {
        self.bin = bin.into();
        self
    }

    fn item_args(&self) -> [&str; 4] {
        ["-a", &self.account, "-s", &self.service]
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Keychain for SecurityCliKeychain {
    fn get_token(&self) -> Result<Option<Secret<String>>, OsError> {
        let out = Command::new(&self.bin)
            .arg("find-generic-password")
            .args(self.item_args())
            .arg("-w")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| OsError::new(format!("cannot run security: {:?}", e.kind())))?;
        match out.status.code() {
            Some(0) => {
                let text = String::from_utf8(out.stdout)
                    .map_err(|_| OsError::new("the stored keychain item is not text"))?;
                let token = text.trim_end_matches(['\n', '\r']).to_string();
                Ok((!token.is_empty()).then(|| Secret::new(token)))
            }
            Some(EXIT_NOT_FOUND) => Ok(None),
            other => Err(OsError::new(format!(
                "security find-generic-password failed (status {other:?})"
            ))),
        }
    }

    fn set_token(&self, token: &Secret<String>) -> Result<(), OsError> {
        let t = token.expose();
        if t.is_empty() || t.len() > MAX_TOKEN_LEN || !t.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(OsError::new(
                "the token must be 1-1024 printable ASCII characters",
            ));
        }
        let mut child = Command::new(&self.bin)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| OsError::new(format!("cannot run security: {:?}", e.kind())))?;
        let command = format!(
            "add-generic-password -a {} -s {} -X {} -U\n",
            self.account,
            self.service,
            hex(t.as_bytes())
        );
        let written = match child.stdin.take() {
            Some(mut stdin) => stdin.write_all(command.as_bytes()),
            None => Err(std::io::Error::other("no stdin")),
        };
        let status = child
            .wait()
            .map_err(|e| OsError::new(format!("security did not finish: {:?}", e.kind())))?;
        if written.is_err() || !status.success() {
            return Err(OsError::new("security add-generic-password failed"));
        }
        // `security -i` can report a failed command in text only, so confirm by reading it back.
        match self.get_token()? {
            Some(back) if back.expose() == t => Ok(()),
            _ => Err(OsError::new("the keychain did not store the token")),
        }
    }

    fn delete_token(&self) -> Result<(), OsError> {
        let status = Command::new(&self.bin)
            .arg("delete-generic-password")
            .args(self.item_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| OsError::new(format!("cannot run security: {:?}", e.kind())))?;
        match status.code() {
            Some(0) | Some(EXIT_NOT_FOUND) => Ok(()),
            other => Err(OsError::new(format!(
                "security delete-generic-password failed (status {other:?})"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_validated() {
        assert!(SecurityCliKeychain::new("my.ijat.x", "acct-1").is_ok());
        for bad in ["", "a b", "a;b", "a'b", "a\nb", "a/b"] {
            assert!(SecurityCliKeychain::new(bad, "a").is_err(), "{bad:?}");
            assert!(SecurityCliKeychain::new("s", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn hex_encodes_bytes() {
        assert_eq!(hex(b"ghp_A"), "6768705f41");
    }
}
