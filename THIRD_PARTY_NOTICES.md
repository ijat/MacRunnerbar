# Third-party notices

| Component | Use | Licence |
|---|---|---|
| RznSans (Regular, Medium) | the app's UI font, `apps/macrunnerbar/assets/RznSans-*.ttf` | OFL 1.1 - `licenses/RznSans-OFL-1.1.txt` |
| LuneUI (`lune-ui`, pinned by git rev) | UI toolkit | MIT OR Apache-2.0 - see its repository; its own notices cover its assets |
| Rust crates | see `Cargo.lock` | each crate's own licence; listing below |

## Rust crate licences

Checked 2026-11-09 with `cargo metadata --filter-platform aarch64-apple-darwin` (204 external crates in the
build graph, every one declares a licence). The binary is built from crates under these SPDX expressions, all
permissive and compatible with this project's `MIT OR Apache-2.0`:

- MIT, Apache-2.0, or both (`MIT OR Apache-2.0`, `Apache-2.0 OR MIT`, `Apache-2.0 AND MIT`): the large majority
- Zlib (alone or in `... OR Zlib OR ...` choices), ISC (`libloading`, `rustls-webpki`, `untrusted`, `ring` as
  `Apache-2.0 AND ISC`), BSD-3-Clause (`subtle`), `BSD-2-Clause OR Apache-2.0 OR MIT` (`zerocopy`), `0BSD OR MIT OR Apache-2.0` (`adler2`)
- `Unlicense OR MIT` (`memchr`, `termcolor`): used under MIT
- `(MIT OR Apache-2.0) AND Unicode-3.0` (`unicode-ident`)
- `Apache-2.0 OR GPL-2.0-only` (`self_cell`): used under Apache-2.0
- `CDLA-Permissive-2.0` (`webpki-roots`, CA root data): permissive data licence

Regenerate with `cargo metadata --format-version 1` and read each package's `license` field.
