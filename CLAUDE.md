# MacRunnerbar

Menu-bar app for macOS arm64, in Rust (edition 2024) on LuneUI: it runs GitHub Actions self-hosted runners
natively on this Mac, one ephemeral JIT runner per queued job, and shows their state. Personal tool for
the user's repos and orgs; macOS arm64 only. No Docker, no Swift/SwiftUI, no App Store or notarization.

## Start here

Read `docs/STATE.md` first. Before stopping, update it (now / next / blockers / last updated) in the
same commit as the work. Decisions live in `docs/DECISIONS.md`; do not re-open one unless the user asks.
`REQUIREMENTS.md` is the product brief: ask the user before deviating from it, and verify every GitHub
API detail against the current docs before relying on it (it lists what is still unverified).

## Commands

Toolchain pinned to `1.92.0` (`rust-toolchain.toml`); lints follow luneui-rs. There is no code yet, so the
cargo and script commands below are not yet runnable (no code); the first plan creates them.

- Build: `cargo build --workspace` (not yet runnable (no code))
- Test (all): `cargo test --workspace` (not yet runnable (no code))
- Test (one): `cargo test -p <crate> <name>` (not yet runnable (no code))
- Format / lint: `cargo fmt --all` / `cargo clippy --workspace --all-targets -- -D warnings` (not yet runnable (no code))
- Boundaries: `scripts/check-boundaries.sh` (not yet runnable (no code))
- Secret scan: `scripts/check-secrets.sh`
- Check (per commit; must be green): `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` (not yet runnable (no code))
- Gate (once per tip before merge; must be green): `scripts/gate.sh` = Check + `scripts/check-boundaries.sh` + `scripts/check-secrets.sh` (not yet runnable (no code))
- Run / package (`.app` bundle with `LSUIElement`, ad-hoc signed): `scripts/bundle.sh` (to be created by the first plan)

Tests never call GitHub and never need a token: the API sits behind a trait with a fake. A live check
against GitHub is a manual step by the user.

## Architecture rules

- Anything external or replaceable (GitHub API, runner release download, runner process, filesystem copy,
  Keychain, settings, launch at login, notifications, sleep/wake, clock, logging sink, UI toolkit) sits behind
  a trait owned by `crates/macrunnerbar-core` (to be created by the first plan); implementations live in
  `crates/macrunnerbar-github` (HTTP) and `crates/macrunnerbar-macos` (OS), both to be created by the first plan.
- One composition root, `apps/macrunnerbar/src/main.rs` (to be created by the first plan), is the only
  crate that builds adapters and depends on `lune-ui`; core produces a `StatusSnapshot`, the app only renders it.
- Dependencies point inward only: adapter -> core, never the reverse. Enforced by `scripts/check-boundaries.sh`
  (to be created by the first plan; the gate runs it once it exists).
- Each trait has a fake and a contract suite in `crates/macrunnerbar-testkit` (to be created by the first
  plan); every implementation passes it.
- `unsafe` only in `macrunnerbar-macos`, each block with `// SAFETY:`; every other crate forbids it.
- Add a new adapter by adding a crate and one registration line in the composition root; core does not change.
- LuneUI is pinned by git `rev` (`32d19048684745b54ab542e5dc886bea43326e08`), as Luneterm does. If it lacks
  something (menu-style list, count badge on the tray icon), tell the user; do not patch it or work around
  it silently.

## Hard rules

1. Never read, print, log, echo or commit the user's PAT (`GITHUB_MASTER_CLASSIC_PAT` in `~/.zshrc`) or any
   token. In the app it lives only in the macOS Keychain, held as `Secret<T>` (prints `[redacted]`). Checks:
   `.claude/settings.json` denies, `scripts/check-secrets.sh`, a sentinel log test (to be created by the first plan).
2. Per-job JIT, ephemeral runners only: nothing idle, a fresh template copy per job, deleted afterwards along
   with any stale registration. Quit or crash leaves no orphan process or registration; cleanup runs on next launch.
3. Runner names use the prefix `ghm-`; cleanup touches only `ghm-*` registrations (`gha-*` and `ghw-*` belong
   to the Linux/Windows autoscaler).
4. Serve a job only if its labels are a subset of `self-hosted,macos,arm64` and include `macos`.
5. Public repos are skipped unless listed in `PUBLIC_ALLOW`: a fork PR would run untrusted code with the user's privileges.
6. Polling uses ETag conditional requests; back off per repo on API errors; survive sleep/wake and network loss.
7. Logs go through `tracing` to a rotating file under `~/Library/Logs/MacRunnerbar/`, no secrets, no job payloads.
8. Every manifest inherits `license = "MIT OR Apache-2.0"` and `publish = false` from the workspace until the user decides.
9. A new third-party dependency needs a one-line justification in `docs/DECISIONS.md`.

## Git

- Worktree per task, always, orchestrated or not (Override: the global file makes this the orchestration default only).
- Commit per change.
- Before the repo is ever published, run the steps in `~/.claude/github/README.md`.

## Read on demand

- `docs/STATE.md` - current work and next steps
- `docs/DECISIONS.md` - decisions and what was rejected
- `REQUIREMENTS.md` - product brief, context, out of scope, what to verify before relying on it
- `~/Projects/luneui-rs/docs/consuming.md` and `docs/components/platform/tray-icon.md` - how to depend on LuneUI; tray and popover
- `~/Projects/infra/runners/README.md` - the Linux/Windows autoscaler whose model this mirrors
