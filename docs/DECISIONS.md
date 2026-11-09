<!-- Append-only. Add a row when a choice has a real alternative. Never edit or delete an old row:
supersede it with a new row that says "supersedes D<n>". Do not re-open a decision unless the user asks. -->
# Decisions

| ID | Date | Decision | Why | Rejected |
|---|---|---|---|---|
| D1 | 2026-11-09 | Rust edition 2024 with LuneUI for all UI; macOS arm64 only | the user's own toolkit; one language | Swift/SwiftUI, Electron/Tauri |
| D2 | 2026-11-09 | Native `actions/runner` osx-arm64, per-job JIT, ephemeral, no idle runners | mirrors the Linux/Windows autoscaler; no stale state | Docker (cannot run macOS jobs), long-lived registered runners |
| D3 | 2026-11-09 | Runner name prefix `ghm-` | no instance cleans up another's registrations | reuse `gha-`/`ghw-` |
| D4 | 2026-11-09 | Classic PAT with `repo` scope, stored in the macOS Keychain | fine-grained PAT is single-owner, cannot span the user's account and orgs | fine-grained PAT, GitHub App (out of scope), plain-file token |
| D5 | 2026-11-09 | Public repos skipped unless in `PUBLIC_ALLOW` | a fork PR would run untrusted code on the Mac | serve all repos |
| D6 | 2026-11-09 | Workspace: `core` (pure) / `github` + `macos` adapters / `testkit` / app (composition root) | swappable interface at every boundary; core tested with a mocked GitHub | single crate |
| D7 | 2026-11-09 | LuneUI pinned by git rev `32d19048684745b54ab542e5dc886bea43326e08`; toolchain `1.92.0` | reproducible; same as Luneterm's pinning and luneui-rs toolchain | path dependency, vendored snapshot, floating branch |
| D8 | 2026-11-09 | License `MIT OR Apache-2.0` (LICENSE-MIT, LICENSE-APACHE) | same as luneui-rs | single license |
| D9 | 2026-11-09 | Workflow: worktree per task always; commit per change | user's choice at init | task-level commits, ask per task |
| D10 | 2026-11-09 | No CI workflow at init; added once the gate exists | a workflow without a gate is red from day 0 | write a placeholder CI |
| D11 | 2026-11-09 | `serde` (derive) is the only dependency of `macrunnerbar-core`: `Settings` derives `Serialize`/`Deserialize`, so every `SettingsStore` shares one on-disk shape | no hand-written mirror struct per adapter; serde is the ecosystem standard and has no I/O | a settings DTO duplicated in the macOS adapter, hand-rolled parsing |
| D12 | 2026-11-09 | Workspace lint table sets `unsafe_code = "forbid"`; `macrunnerbar-macos` (step 3) defines its own `[lints]` table without it instead of inheriting | a new crate is unsafe-free by default; the one crate that may use `unsafe` opts out explicitly, each block with `// SAFETY:` | per-crate `#![forbid(unsafe_code)]` (a forgotten line silently allows unsafe) |
| D13 | 2026-11-09 | `ureq` 3 (`default-features = false`, `rustls` only) is the HTTP client in `macrunnerbar-github` | small blocking client, enough for ~2 polls/min and one download; no gzip, JSON or cookie features pulled in; rustls (ring, bundled roots) avoids a system TLS dependency | `reqwest` + `tokio` (async runtime for nothing), shelling out to `curl` |
| D14 | 2026-11-09 | `serde_json` in `macrunnerbar-github`, parsed to `Value` and read field by field | GitHub payloads are large and mostly unused; no DTO structs to keep in sync; parse errors carry no payload text | typed `Deserialize` structs per endpoint |
| D15 | 2026-11-09 | `sha2` in `macrunnerbar-github` to verify the runner release SHA-256 while streaming the download | required by the plan; RustCrypto, pure Rust | `shasum` subprocess, no verification |
| D16 | 2026-11-09 | `tracing` facade only (no subscriber) in `macrunnerbar-github` for the debug log of JIT `runner.labels` | adapters log through the facade; the app installs the rotating-file subscriber (step 3/5) | `log` crate, no logging in adapters |
