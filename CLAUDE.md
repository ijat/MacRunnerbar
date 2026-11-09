# MacRunnerbar

Menu-bar app for macOS arm64, in Rust (edition 2024) on LuneUI: it runs GitHub Actions self-hosted runners
natively on this Mac, one ephemeral JIT runner per queued job, and shows their state. Personal tool for
the user's repos and orgs; macOS arm64 only. No Docker, no Swift/SwiftUI, no App Store or notarization.

## Start here

Read `docs/STATE.md` first. Before stopping, update it (now / next / blockers / last updated) in the
same commit as the work. Decisions live in `docs/DECISIONS.md`; do not re-open one unless the user asks.
Verify every GitHub API detail against the current docs before relying on it; `docs/STATE.md` lists what
is still unverified.

## Commands

Toolchain pinned to `1.92.0` (`rust-toolchain.toml`); lints follow [luneui-rs](https://github.com/lune-rs/luneui-rs).

- Build: `cargo build --workspace`
- Test (all): `cargo test --workspace`
- Test (one): `cargo test -p <crate> <name>` (engine: `cargo test -p macrunnerbar-core engine`)
- Format / lint: `cargo fmt --all` / `cargo clippy --workspace --all-targets -- -D warnings`
- Boundaries: `scripts/check-boundaries.sh` (from `cargo metadata` and `rustc -F unsafe_code`)
- Secret scan: `scripts/check-secrets.sh` (needs `gitleaks`)
- Check (per commit; must be green): `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- Gate (once per tip before merge; must be green): `scripts/gate.sh` = Check + `scripts/check-boundaries.sh` + `scripts/check-secrets.sh`; CI (`.github/workflows/ci.yml`) runs it on `macos-latest`, then `scripts/bundle.sh`
- Run / package (`.app` bundle with `LSUIElement`, ad-hoc signed): `scripts/bundle.sh` (`--check` also verifies the plist and signature and runs `macrunnerbar --smoke`, which needs a GUI session); look without a display: `macrunnerbar --dump <dir>` writes the popover, Preferences and icons as PNG
- Manual, real login Keychain (scratch item, not in the gate): `cargo test -p macrunnerbar-macos -- --ignored`

Tests never call GitHub and never need a token: the API sits behind a trait with a fake. A live check
against GitHub is a manual step by the user.

## Architecture rules

- Anything external or replaceable (GitHub API, runner release download, runner process, filesystem copy,
  Keychain, settings, launch at login, notifications, sleep/wake, clock, logging sink, UI toolkit) sits behind
  a trait owned by `crates/macrunnerbar-core`; implementations live in
  `crates/macrunnerbar-github` (HTTP) and `crates/macrunnerbar-macos` (OS).
- One composition root, `apps/macrunnerbar` (`src/wiring.rs`), is the only
  crate that builds adapters and depends on `lune-ui`; core produces a `StatusSnapshot`, the app only renders it.
- Dependencies point inward only: adapter -> core, never the reverse. Enforced by `scripts/check-boundaries.sh`
  (part of the gate). Core's only dev-dependency on a workspace crate is the testkit, used from `tests/`.
- Each trait has a fake and a contract suite in `crates/macrunnerbar-testkit`; every implementation passes it.
- `unsafe` only in `macrunnerbar-macos`, each block with `// SAFETY:`; every other crate forbids it.
- Add a new adapter by adding a crate and one registration line in the composition root; core does not change.
- LuneUI is pinned by git `rev` (`32d19048684745b54ab542e5dc886bea43326e08`). If it lacks
  something (menu-style list, count badge on the tray icon), tell the user; do not patch it or work around
  it silently.

## Hard rules

1. Never read, print, log, echo or commit the user's PAT (`GITHUB_MASTER_CLASSIC_PAT` in their shell profile) or any
   token. In the app it lives only in the macOS Keychain, held as `Secret<T>` (prints `[redacted]`). The Keychain does not hide it from jobs (they run as the user and can call `security`, D31); do not claim it does. Checks:
   `.claude/settings.json` denies, `scripts/check-secrets.sh`, sentinel tests (`macrunnerbar-macos` log and Keychain argv, `macrunnerbar-github` errors, the app's Preferences form).
2. Per-job JIT, ephemeral runners only: nothing idle, a fresh template copy per job, deleted afterwards along
   with any stale registration. Quit or crash leaves no orphan process or registration; cleanup runs on next launch.
3. Runner names use the prefix `ghm-`; cleanup touches only `ghm-*` registrations (other runner setups use other prefixes).
4. Serve a job only if its labels are a subset of `self-hosted,macos,arm64` and include `macos`.
5. Public repos are skipped unless listed in `PUBLIC_ALLOW`: a fork PR would run untrusted code with the user's privileges.
6. Polling uses ETag conditional requests; back off per repo on API errors; survive sleep/wake and network loss.
7. Logs go through `tracing` to a rotating file under `~/Library/Logs/MacRunnerbar/`, no secrets, no job payloads.
8. Every manifest inherits `license = "MIT OR Apache-2.0"` and `publish = false` from the workspace until the user decides.
9. A new third-party dependency needs a one-line justification in `docs/DECISIONS.md`.

## Git

- Worktree per task, always, orchestrated or not (Override: the global file makes this the orchestration default only).
- Commit per change.

## Read on demand

- `docs/STATE.md` - current work and next steps
- `docs/DECISIONS.md` - decisions and what was rejected
- LuneUI: <https://github.com/lune-rs/luneui-rs> (`docs/consuming.md`, `docs/components/platform/tray-icon.md`) - how to depend on it; tray and popover
