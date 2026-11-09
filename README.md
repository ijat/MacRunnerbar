# MacRunnerbar

Menu-bar app for macOS arm64, written in Rust on LuneUI, that runs GitHub Actions self-hosted runners
natively on this Mac: one ephemeral JIT runner per queued job, with their state shown in the menu bar.

Status: pre-alpha, no code yet. See `REQUIREMENTS.md` for the product brief and `docs/STATE.md` for
current work.

## Commands (planned)

Not yet runnable (no code); the first plan creates them.

- Build: `cargo build --workspace`
- Test: `cargo test --workspace`
- Format / lint: `cargo fmt --all` / `cargo clippy --workspace --all-targets -- -D warnings`
- Full gate: `scripts/gate.sh`
- Secret scan: `scripts/check-secrets.sh` (needs `gitleaks`)

## Security

Jobs run with your user's privileges; an optional dedicated non-admin user is planned. The token is
kept only in the macOS Keychain. Public repos are skipped by default.

## License

`MIT OR Apache-2.0` (see `LICENSE-MIT` and `LICENSE-APACHE`).
