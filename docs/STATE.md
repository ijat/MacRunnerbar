# State

Last updated: 2026-11-09 (v1 merged, fd4e6e0)

## Now

v1 is merged on `main` (workspace, GitHub and macOS adapters, engine, app, gate, CI). Gate parts green
(fmt, clippy, tests, boundaries, secrets). Not yet run against real GitHub. Publishing the repo is next.

## Next

1. The user's live check (README, "First run and live check"): token in Preferences, a real queued
   `[self-hosted, macos, arm64]` job, tray states, Quit and crash cleanup, launch at login. It also settles two
   assumptions: JIT runner labels versus `runs-on`, and `run.sh --jitconfig` (an environment-variable form exists,
   `ProcessRunnerHost::with_jit_via_env`).
2. Run `cargo test -p macrunnerbar-macos -- --ignored` once by hand (real login Keychain, scratch item).
3. Publish (separate, approved step): first the public-readiness clean-up (home paths, private repo/infra
   names, stale REQUIREMENTS.md header; see the refresh findings), then `~/.claude/github/README.md`
   (collaborators-only PRs, push owner-only) and `scripts/check-secrets.sh`; check the font and crate
   licences in `THIRD_PARTY_NOTICES.md`.

## Blockers / open questions

- None blocking the merge. Open: whether LuneUI should grow a list-of-rows widget and glyph-free checkbox (the app
  draws its own list and uses a toggle); two Macs using the `ghm-` prefix on one repo would sweep each other's
  registrations (a single Mac is assumed).
