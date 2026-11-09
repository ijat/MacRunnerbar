//! Keychain (against a fake `security` binary), LaunchAgent, logging, notifier, settings, clock.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use macrunnerbar_core::{Keychain, LoginItem, Notification, Notifier, Secret, SettingsStore};
use macrunnerbar_macos::{
    JsonSettingsStore, LaunchAgent, OsascriptNotifier, Paths, SecurityCliKeychain, SystemClock,
    logging,
};
use macrunnerbar_testkit::contract;
use support::Scratch;
use tracing::Level;

// Split so the source holds no token-shaped literal for the secret scanner.
const SENTINEL: &str = concat!("gh", "p_SENTINEL0123456789abcdefABCDEF0123456789");

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/bash\n{body}\n")).expect("write script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// A `security` stand-in with the real tool's shape: `-i` reads commands on stdin, `find-generic-password`
/// prints the secret, exit 44 means "not found". It logs every argument it is given.
const FAKE_SECURITY: &str = r#"d="$(cd "$(dirname "$0")" && pwd)"
printf '%s\n' "$@" >> "$d/argv.log"
case "$1" in
  -i)
    input="$(cat)"
    printf '%s\n' "$input" >> "$d/stdin.log"
    hex="$(printf '%s' "$input" | sed -n 's/.* -X \([0-9a-f]*\)\( .*\)*$/\1/p')"
    printf '%s' "$hex" | xxd -r -p > "$d/item"
    exit 0;;
  find-generic-password)
    [ -f "$d/item" ] || exit 44
    cat "$d/item"; echo; exit 0;;
  delete-generic-password)
    [ -f "$d/item" ] || exit 44
    rm "$d/item"; exit 0;;
esac
exit 1"#;

fn fake_keychain(scratch: &Scratch, body: &str) -> (SecurityCliKeychain, PathBuf) {
    let bin = write_script(scratch.path(), "security", body);
    (
        SecurityCliKeychain::new("my.ijat.macrunnerbar.test", "github-pat")
            .expect("ident")
            .with_binary(&bin),
        bin,
    )
}

#[test]
fn keychain_passes_the_contract_against_a_fake_security_binary() {
    let scratch = Scratch::new("kc-contract");
    let (kc, _) = fake_keychain(&scratch, FAKE_SECURITY);
    contract::keychain_contract(&kc);
}

#[test]
fn keychain_never_puts_the_token_in_an_argument_list() {
    let scratch = Scratch::new("kc-argv");
    let (kc, _) = fake_keychain(&scratch, FAKE_SECURITY);
    kc.set_token(&Secret::new(SENTINEL.into())).expect("set");
    assert_eq!(
        kc.get_token().expect("get").expect("present").expose(),
        SENTINEL
    );
    kc.delete_token().expect("delete");

    let argv = std::fs::read_to_string(scratch.path().join("argv.log")).expect("argv log");
    let hex: String = SENTINEL.bytes().map(|b| format!("{b:02x}")).collect();
    assert!(
        !argv.contains(SENTINEL),
        "the token appeared in an argument"
    );
    assert!(
        !argv.contains(&hex),
        "the hex-encoded token appeared in an argument"
    );
    assert!(
        !argv.contains("ghp_"),
        "something token-shaped appeared in an argument"
    );
    // The add command went through stdin of `security -i`, in hex.
    let stdin = std::fs::read_to_string(scratch.path().join("stdin.log")).expect("stdin log");
    assert!(
        stdin.starts_with("add-generic-password -a github-pat -s my.ijat.macrunnerbar.test -X ")
    );
    assert!(stdin.contains(&hex));
    assert!(argv.lines().any(|l| l == "-i"));
}

#[test]
fn keychain_errors_never_contain_the_token() {
    let scratch = Scratch::new("kc-errors");
    let secret = Secret::new(SENTINEL.to_string());
    // security fails outright.
    let (failing, _) = fake_keychain(&scratch, "exit 1");
    let e = failing.set_token(&secret).expect_err("set fails");
    assert!(!format!("{e} {e:?}").contains(SENTINEL));
    let e = failing.get_token().expect_err("get fails");
    assert!(!format!("{e} {e:?}").contains(SENTINEL));
    // `security -i` exits 0 without storing: caught by the read-back.
    let noop = Scratch::new("kc-noop");
    let (forgetful, _) = fake_keychain(
        &noop,
        "case \"$1\" in -i) cat >/dev/null; exit 0;; *) exit 44;; esac",
    );
    let e = forgetful
        .set_token(&secret)
        .expect_err("read-back must catch it");
    assert!(!format!("{e} {e:?}").contains(SENTINEL));
    // A missing binary.
    let missing = SecurityCliKeychain::standard().with_binary("/nonexistent/security");
    let e = missing.set_token(&secret).expect_err("no binary");
    assert!(!format!("{e} {e:?}").contains(SENTINEL));
}

#[test]
fn keychain_rejects_tokens_it_cannot_pass_safely() {
    let scratch = Scratch::new("kc-reject");
    let (kc, _) = fake_keychain(&scratch, FAKE_SECURITY);
    for bad in ["", "has space", "new\nline", "tab\t", "caf\u{e9}"] {
        assert!(kc.set_token(&Secret::new(bad.into())).is_err(), "{bad:?}");
    }
    assert!(
        !scratch.path().join("argv.log").exists(),
        "security must not even be run"
    );
}

#[test]
fn launch_agent_passes_the_login_item_contract_and_round_trips() {
    let scratch = Scratch::new("agent");
    let dir = scratch.path().join("LaunchAgents");
    let exe = Path::new("/Applications/MacRunnerbar.app/Contents/MacOS/macrunnerbar");
    let agent = LaunchAgent::new(&dir, exe).expect("agent");
    contract::login_item_contract(&agent);

    assert!(agent.read().expect("read").is_none());
    agent.set_enabled(true).expect("enable");
    let spec = agent.read().expect("read").expect("installed");
    assert_eq!(spec.label, "my.ijat.macrunnerbar");
    assert_eq!(
        spec.program_arguments,
        vec![exe.to_string_lossy().to_string()]
    );
    assert!(spec.run_at_load);
    assert!(agent.is_enabled().expect("enabled"));
    // The file is a valid property list as far as macOS is concerned.
    let lint = Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(agent.plist_path())
        .output()
        .expect("plutil");
    assert!(lint.status.success(), "plutil rejected the plist");
    // Rewriting is fine; removing deletes the file; removing again is fine.
    agent.set_enabled(true).expect("enable again");
    agent.set_enabled(false).expect("disable");
    assert!(!agent.plist_path().exists());
    assert!(agent.read().expect("read").is_none());
    assert!(!agent.is_enabled().expect("disabled"));
    agent.set_enabled(false).expect("disable again");
    assert!(
        std::fs::read_dir(&dir).expect("dir").next().is_none(),
        "no temp file left behind"
    );
}

#[test]
fn launch_agent_needs_an_absolute_executable_and_survives_special_characters() {
    let scratch = Scratch::new("agent-paths");
    assert!(LaunchAgent::new(scratch.path(), Path::new("relative/app")).is_err());
    let exe = Path::new("/Applications/Mac & <Runner>.app/Contents/MacOS/x");
    let agent = LaunchAgent::new(scratch.path(), exe).expect("agent");
    agent.set_enabled(true).expect("enable");
    assert_eq!(
        agent.read().expect("read").expect("spec").program_arguments[0],
        exe.to_string_lossy()
    );
    let lint = Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(agent.plist_path())
        .output()
        .expect("plutil");
    assert!(lint.status.success());
}

fn read_log(dir: &Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).expect("log dir").flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(
            name.starts_with("macrunnerbar") && name.ends_with(".log"),
            "unexpected log file {name}"
        );
        out.push_str(&std::fs::read_to_string(entry.path()).expect("read log"));
    }
    out
}

#[test]
fn log_file_never_contains_the_token_even_when_code_logs_it() {
    let scratch = Scratch::new("log");
    let dir = scratch.path().join("Logs");
    let (sub, redactor) = logging::subscriber(&dir, Level::DEBUG).expect("subscriber");
    let typed = Secret::new("hunter2-typed-in-preferences".to_string());
    redactor.add(&typed);
    let secret = Secret::new(SENTINEL.to_string());
    tracing::subscriber::with_default(sub, || {
        tracing::info!("startup ok");
        tracing::info!(token = SENTINEL, "a careless field");
        tracing::warn!("auth header was Authorization: Bearer {SENTINEL}");
        tracing::debug!(token = ?secret, "a Secret in a field");
        tracing::error!("typed value {} leaked", typed.expose());
        tracing::info!(repo = "o/r", runner = "ghm-o-r-abc123", "runner started");
    });
    let log = read_log(&dir);
    assert!(
        log.contains("startup ok") && log.contains("ghm-o-r-abc123"),
        "ordinary lines are kept"
    );
    assert!(
        !log.contains(SENTINEL),
        "the sentinel token reached the log file"
    );
    assert!(
        !log.contains("ghp_"),
        "something token-shaped reached the log file"
    );
    assert!(
        !log.contains("hunter2-typed-in-preferences"),
        "a registered secret reached the log file"
    );
    assert!(log.contains("[redacted]"));
    assert_eq!(
        std::fs::read_dir(&dir).expect("dir").count(),
        1,
        "one rotating file so far"
    );
}

#[test]
fn notifier_passes_title_and_body_as_arguments_not_script_text() {
    let scratch = Scratch::new("notify");
    let bin = write_script(
        scratch.path(),
        "osascript",
        "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv.log\"",
    );
    let n = OsascriptNotifier::default().with_binary(&bin);
    contract::notifier_contract(&n);

    let hostile = "\" & (do shell script \"touch pwned\") & \"\nsecond line";
    n.notify(&Notification {
        title: "Job failed".into(),
        body: hostile.into(),
    })
    .expect("notify");
    let argv = std::fs::read_to_string(scratch.path().join("argv.log")).expect("argv log");
    let lines: Vec<&str> = argv.lines().collect();
    // The script text is fixed; user text only ever appears after `--`.
    let dashes = lines.iter().position(|l| *l == "--").expect("separator");
    assert!(
        lines[..dashes]
            .iter()
            .all(|l| !l.contains("pwned") && !l.contains("Job failed"))
    );
    assert_eq!(
        lines[dashes + 1],
        "\" & (do shell script \"touch pwned\") & \"second line"
    );
    assert_eq!(lines[dashes + 2], "Job failed");
    assert!(!scratch.path().join("pwned").exists());
}

#[test]
fn notifier_reports_a_failing_osascript() {
    let scratch = Scratch::new("notify-fail");
    let bin = write_script(scratch.path(), "osascript", "exit 1");
    let n = OsascriptNotifier::default().with_binary(&bin);
    assert!(
        n.notify(&Notification {
            title: "t".into(),
            body: "b".into()
        })
        .is_err()
    );
}

#[test]
fn json_settings_store_passes_the_contract_and_rejects_corrupt_files() {
    let scratch = Scratch::new("settings");
    let paths = Paths::under(scratch.path());
    let store = JsonSettingsStore::new(&paths);
    contract::settings_store_contract(&store);
    // Unknown or missing fields fall back to defaults; garbage is an error, not a silent reset.
    std::fs::write(paths.settings_file(), r#"{"global_max": 5}"#).expect("write");
    let s = store.load().expect("partial file");
    assert_eq!((s.global_max, s.per_owner_max), (5, 2));
    std::fs::write(paths.settings_file(), "not json").expect("write");
    assert!(store.load().is_err());
    let text = std::fs::read_to_string(paths.settings_file()).expect("read");
    assert_eq!(text, "not json", "a failed load leaves the file alone");
}

#[test]
fn system_clock_passes_the_contract() {
    contract::clock_contract(&SystemClock);
}

/// Touches the real login Keychain with a scratch item that this test creates and deletes by its own
/// unique service name. Not run by default.
#[test]
#[ignore = "uses the real login Keychain; run by hand: cargo test -p macrunnerbar-macos -- --ignored"]
fn real_keychain_round_trip_with_a_scratch_item() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let service = format!(
        "my.ijat.macrunnerbar.scratch-{}-{nanos}",
        std::process::id()
    );
    let kc = SecurityCliKeychain::new(&service, "scratch").expect("ident");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract::keychain_contract(&kc)
    }));
    kc.delete_token().expect("remove the scratch item");
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}
