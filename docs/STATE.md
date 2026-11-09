# State

Last updated: 2026-11-09 (v1 step 4)

## Now

v1 orchestration in progress on branch `orch/v1`. Done: workspace + core/testkit (step 1), GitHub adapter (2), macOS adapter (3), engine in `macrunnerbar-core::engine` (4), job-conclusion notifications (step 5, Revision 1). Pending: app with tray/popover/preferences/bundle (5), gate/boundaries/CI/docs (6).

## Next

1. Ask the user the open questions: which owners/repos to serve first; logged-in user or a dedicated local user.
2. First plan: workspace skeleton (`Cargo.toml` with workspace lints, `crates/macrunnerbar-{core,github,macos,testkit}`,
   `apps/macrunnerbar`), traits and fakes, `scripts/check-boundaries.sh`, `scripts/gate.sh`; verify the GitHub JIT
   details (labels set at creation, `run.sh --jitconfig`) and pick the launch-at-login mechanism.
3. Add CI once the gate exists: `.github/workflows/ci.yml` running `scripts/gate.sh`, on a macOS arm64 runner
   (the app crate depends on macOS-only code); pin `actions/checkout` and `Swatinem/rust-cache` to their latest majors then.
4. Before publishing the repo: run `~/.claude/github/README.md` (collaborators-only PRs, push owner-only) and re-scan history with `scripts/check-secrets.sh`.

## Blockers / open questions

- Owners/repos to serve first; run jobs as logged-in or dedicated user (REQUIREMENTS.md open questions).
- Whether LuneUI can draw a count badge into the tray icon and render a menu-style list (to be checked in the first plan).
