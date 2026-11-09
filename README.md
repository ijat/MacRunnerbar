# MacRunnerbar

A menu-bar app for macOS (Apple silicon) that runs GitHub Actions self-hosted runners natively on your
Mac: one ephemeral just-in-time runner per queued job, nothing idle, with their state in the menu bar.
Written in Rust (edition 2024) on [LuneUI](https://github.com/lune-rs/luneui-rs).

Status: pre-alpha. The code is tested against fakes and a local HTTP stub; it has not yet been run
against real GitHub (see [First run](#first-run-and-live-check)).

## What it does

- Polls the repos your token administers for queued jobs whose labels fit `self-hosted, macos, arm64`
  (and include `macos`, so a bare `runs-on: self-hosted` stays on your other runners).
- For each queued job, within your caps (default 2 runners per owner and overall), mints a JIT runner
  configuration, copies a fresh runner directory, runs it once, then deletes the directory and the
  registration. The runner template (`actions/runner`, osx-arm64) is refreshed daily and checked against
  its published SHA-256.
- Menu-bar icon: idle, running with the number of running jobs drawn in, error, paused. Click it for
  the running jobs (repo, workflow / job, elapsed time, a link to the run), jobs held back by a cap,
  `n/max` runners, the last error, Pause / Resume, Preferences and Quit.
- Notifies when a runner fails or a job ends red (failure or timeout), by default; all or none on request.
- Survives sleep and network loss, backs off per repo, and cleans up after a crash on the next launch.
- Starts at login if you ask (a LaunchAgent).

## Security

- **Jobs run as you**, with your user's privileges, in a per-job directory that is deleted afterwards.
  Anything a workflow does, it does as your account. Only run workflows you trust.
- **Public repos are skipped** unless you list them under "Public repos to serve": a pull request from a
  fork would run untrusted code on your Mac.
- The token (a classic PAT with `repo` scope; the repos must be ones you administer) is stored in the
  macOS Keychain and held in memory as a redacting `Secret`. It never appears in a process argument, a log
  line, an error message or the settings file.
- **The Keychain does not protect the token from your jobs.** The item is created through the `security`
  command-line tool, which macOS trusts to read it without a prompt, and jobs run as you with `/usr/bin` on
  their `PATH`: a workflow step (or an action a step pulls in) can run
  `security find-generic-password -s my.ijat.macrunnerbar -a github-pat -w` and get the token while your
  login keychain is unlocked. Treat the token like anything else a job can reach. Give it only the access
  you need: serve only repos you trust (an owner or an org where untrusted people can get a workflow to
  run is a way to your token), and, if you serve a single owner, prefer a fine-grained token limited to
  those repos (repository permissions Administration read and write for runner registration, Actions read,
  Metadata read; not tried here) over a classic one. Revoke and replace it if a job you did not trust ran.
- Runner processes get a minimal environment (`HOME`, `USER`, `PATH`, locale), not the app's: a token
  in your shell environment does not reach a job.
- Only runners named `ghm-*` are ever created, listed or deleted; other runners'
  registrations are never touched.

## Build and run

Requires macOS on Apple silicon and the toolchain in `rust-toolchain.toml` (rustup installs it).

```sh
scripts/bundle.sh          # target/bundle/MacRunnerbar.app (release build, no Dock icon, ad-hoc signed)
scripts/bundle.sh --check  # also verifies the plist and signature, and starts and stops the app once
```

Copy the app to `/Applications` and open it. (Turn on "Start at login" from the copy you want to keep:
the login item points at the executable it is switched on from.)

## First run and live check

The first run against real GitHub is a manual check. Use a private repo you administer.

1. Open the app, click the menu-bar icon, choose Preferences, paste the token, press Save next to it.
   Under "Only these" list the one test repo, then press Save.
2. In that repo add `.github/workflows/mac-smoke.yml`:

   ```yaml
   name: mac-smoke
   on: workflow_dispatch
   jobs:
     smoke:
       runs-on: [self-hosted, macos, arm64]
       steps:
         - run: echo "ran as $(whoami) on $(uname -m)"
   ```

   and start it (`gh workflow run mac-smoke.yml`, or the Actions tab).
3. Within a poll interval (30 s by default) the icon becomes a filled square with a `1`, the popover lists
   the job with a link, and the job finishes green. Afterwards the job directory is gone
   (`ls ~/Library/Application\ Support/MacRunnerbar/jobs`) and the repo's Settings, Actions, Runners shows
   no `ghm-*` runner. Logs are in `~/Library/Logs/MacRunnerbar/`.
4. Put `exit 1` in the step: you get a "Job failed" notification.
5. Start the job again and Quit from the popover while it runs: no `Runner.Listener` process remains
   (`pgrep -fl Runner.Listener`) and the jobs directory is empty. Start it once more, `kill -9` the app,
   relaunch: the leftovers are removed at start.
6. Switch "Start at login" on, log out and in again: the icon comes back.

Not yet confirmed against real GitHub: that a runner registered with the labels `self-hosted, macos,
arm64` is the one that takes a `runs-on: [self-hosted, macos, arm64]` job (the Linux autoscaler this
mirrors relies on the same behaviour), and that `run.sh --jitconfig <encoded>` starts a runner that
exits after one job. If the job is never picked up, look at the runner's labels in the repo's Runners
page while it is registered. The host can pass the configuration through the environment instead of
the argument list (`ProcessRunnerHost::with_jit_via_env`, which keeps it out of `ps`) once that is
confirmed too.

## Development

```sh
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
scripts/gate.sh            # the above + scripts/check-boundaries.sh + scripts/check-secrets.sh (needs gitleaks)
macrunnerbar --dump <dir>  # render the popover, Preferences and icons to PNG, without a display
```

Layout (details in `CLAUDE.md`, decisions in `docs/DECISIONS.md`):

| crate | what |
|---|---|
| `macrunnerbar-core` | domain types, the traits for everything external, pure policy (labels, caps, backoff), the engine |
| `macrunnerbar-github` | the GitHub REST adapter (blocking `ureq`, ETag cache, JIT runners, job lookups, release download) |
| `macrunnerbar-macos` | runner processes and directories, Keychain, LaunchAgent, rotating redacted log, notifications, settings |
| `macrunnerbar-testkit` | fakes and one contract suite per trait |
| `macrunnerbar-app` (`apps/macrunnerbar`) | the composition root, tray, popover and Preferences on LuneUI |

Tests never touch GitHub or the Keychain. CI (`.github/workflows/ci.yml`) runs the gate on a
GitHub-hosted macOS runner.

## License

`MIT OR Apache-2.0` (see `LICENSE-MIT` and `LICENSE-APACHE`). RznSans (the UI font) is under the SIL
OFL 1.1, see `THIRD_PARTY_NOTICES.md`.
