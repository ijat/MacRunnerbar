# MacRunnerbar — requirements

A small native macOS menu bar app that runs **GitHub Actions self-hosted runners directly on this Mac** (macOS, arm64)
and shows what they are doing. Not started yet: this file is the brief for a fresh agent. Ask the user before
deviating from it; verify every GitHub API detail against the current docs before relying on it.

## Goal

- macOS jobs (`runs-on: [self-hosted, macos, arm64]`) run on the user's Mac with no manual runner babysitting.
- The menu bar icon tells at a glance: idle / N jobs running / error / paused.
- No Docker: a container can't run macOS jobs. Runners are the native `actions/runner` `osx-arm64` build.

## Context (existing home-lab setup, for consistency)

- Linux and Windows runners already exist: `~/Projects/infra/runners/` (`autoscaler.py`, `README.md`). Read its README
  first and mirror its model: **per-job JIT runner, nothing idle, ephemeral**, same repo scan, same caps idea.
- Existing runner names: `gha-*` (Linux), `ghw-*` (Windows). Use a new prefix, e.g. `ghm-*`, so no instance
  cleans up another's registrations.
- Auth there is a **classic PAT with `repo` scope** (a fine-grained PAT is single-owner, so it can't cover the user's
  account plus several orgs). The user's token for automation is `GITHUB_MASTER_CLASSIC_PAT` in `~/.zshrc`:
  never print it, log it or commit it. In the app, store the token in the **macOS Keychain**, never in plain files.
- Public repos are skipped by default (a fork PR would run untrusted code on the Mac); opt-in list `PUBLIC_ALLOW`.
- Polling uses ETag conditional requests (a 304 doesn't count against the rate limit).

## Functional requirements

1. **Runner management**
   - Poll the watched repos for queued jobs whose labels are a subset of `self-hosted,macos,arm64`
     (and require `macos`, so a bare `self-hosted` job stays on Linux).
   - For each queued job: mint a JIT config (`POST /repos/{repo}/actions/runners/generate-jitconfig`), start an
     `actions/runner` from a fresh copy of a template dir, let it take one job and exit, then delete the dir and
     any stale registration.
   - Caps: configurable per-owner and global max concurrent runners (default 2).
   - Auto-update the runner template daily (latest `actions/runner` osx-arm64, check the SHA-256).
   - Run jobs as the current user, in a per-job work dir deleted afterwards. Document the security implication
     (jobs run with the user's privileges) and offer an optional dedicated non-admin local user as a setting.
2. **Menu bar UI** (LuneUI tray item + popover window, no Dock icon, `LSUIElement`; LuneUI's tray icon is a CPU raster,
   so state and count are drawn into the icon image)
   - Icon states: idle, running (with count badge), error, paused.
   - Menu: running jobs (repo, workflow, job name, elapsed time, link that opens the run in the browser);
     queued jobs waiting on caps; runner count `n/max`; last error; Pause/Resume; Preferences; Quit.
   - Optional notification when a job finishes, failed jobs only by default.
3. **Preferences**
   - PAT (Keychain), repo scope (all repos the PAT can see, minus excludes; the include/exclude and `PUBLIC_ALLOW`
     lists), caps, poll interval, label set, launch at login (LaunchAgent or `SMAppService`, whichever is simpler from Rust).
4. **Operation**
   - Survives sleep/wake and network loss; backs off on API errors (per-repo, like the existing autoscaler).
   - On quit or crash: don't leave orphan runner processes or registrations; clean up on next launch.
   - Logs via `tracing` (or `log`) to a rotating file under `~/Library/Logs/MacRunnerbar/`. No secrets in logs.

## Non-functional

- **Rust** (edition 2024) with **LuneUI** (`~/Projects/luneui-rs`, the user's own toolkit) for all UI. No Swift/SwiftUI.
  macOS arm64 only.
- Read first: LuneUI's `README.md`, `CLAUDE.md`, `docs/consuming.md` (how to depend on it) and its tray/popover docs
  (`docs/components/platform/tray-icon.md`, `crates/lune-ui-platform/src/{tray,popover}.rs`). `apps/gallery`
  (`window.rs`, `popover.rs`) is the worked example of the app-owned event loop and a tray host.
  Pin `lune-ui` by git `rev`, as `~/Projects/Luneterm/Cargo.toml` does. If LuneUI lacks something (e.g. a menu-style
  list, a status badge on the tray icon), say so to the user rather than patching LuneUI or working around it silently.
- UI-independent core crate for the runner logic (GitHub API behind a trait, mocked in tests); a thin app crate on top
  with the tray, popover and preferences. Follow LuneUI's conventions (lints, `rust-toolchain.toml`).
- Dependencies: keep few (HTTP client, JSON, async runtime or threads; Keychain via a small crate or `security` CLI).
  Justify each.
- Signed locally (ad-hoc is fine); no App Store, no notarization needed. Build with `cargo` from the shell.
- Public repo hygiene (if it's ever published): run `~/.claude/github/README.md` steps first.

## Out of scope

Linux/Windows runners, Docker, Windows/Linux builds of the app, org-level runner groups, GitHub App auth, auto-installing Xcode toolchains.

## Open questions for the user (ask before building)

- Which owners/repos should it serve first?
- Run jobs as the logged-in user or a dedicated local user?
- Name is settled: `MacRunnerbar`.

## Verify before relying on

- JIT config API shape and how to run the runner with it (`run.sh --jitconfig`).
- Whether the `Luneterm` org and the user's other orgs allow repo-level runners and classic PATs.
- That `[self-hosted, macos, arm64]` matches what the runner auto-registers for with JIT (labels are set at JIT creation).
