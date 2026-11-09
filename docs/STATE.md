# State

Last updated: 2026-11-09 (v1 step 6)

## Now

v1 is implemented on branch `orch/v1` (workspace, GitHub and macOS adapters, engine, app, gate, CI) and waits
for review and the merge. Last updated: step 6 (gate, boundaries, CI, README).

## Next

1. The user's live check (README, "First run and live check"): token in Preferences, a real queued
   `[self-hosted, macos, arm64]` job, tray states, Quit and crash cleanup, launch at login. It also settles two
   assumptions: JIT runner labels versus `runs-on`, and `run.sh --jitconfig` (an environment-variable form exists,
   `ProcessRunnerHost::with_jit_via_env`).
2. Run `cargo test -p macrunnerbar-macos -- --ignored` once by hand (real login Keychain, scratch item).
3. Before publishing the repo (a separate, approved step): `~/.claude/github/README.md` (collaborators-only PRs,
   push owner-only) and `scripts/check-secrets.sh`; check the font and crate licences listed in `THIRD_PARTY_NOTICES.md`.

## Blockers / open questions

- None blocking the merge. Open: whether LuneUI should grow a list-of-rows widget and glyph-free checkbox (the app
  draws its own list and uses a toggle); two Macs using the `ghm-` prefix on one repo would sweep each other's
  registrations (a single Mac is assumed).
