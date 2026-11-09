# State

Last updated: 2026-11-09 (v1 step 1)

## Now

v1 orchestration in progress on branch `orch/v1`. Step 1 done: workspace, `macrunnerbar-core` (types, traits, policy), `macrunnerbar-testkit` (fakes, contract suites). Steps 2-6 pending (GitHub adapter, macOS adapter, engine, app, gate/CI).

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
