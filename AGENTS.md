# Rules for coding agents

Read this file before you change anything in this repo. These rules apply to every agent and
every contributor.

## Start here

1. If `nextsession.txt` exists, read it and do the next open step of its PLAN. Keep the scope to that step.
2. For UI work, read `docs/design/screens/README.md`. It is the accepted design.
3. Older documents in `docs/design/` and `docs/adr/` give background. The accepted design wins
   where they disagree.

## Layout

The Rust workspace is in `Cargo.toml`. See [README.md](README.md) for the folder table.

- `crates/*` are libraries and the daemon `aivoryd`. They are the `default-members`.
- `apps/desktop` is the GPUI app (crate `aivory-desktop`, binary `aivory`).
  It is a workspace member, but not a default member.
- `apps/mobile` is the Flutter app. It is not part of the Cargo workspace.
- `proto/` holds the JSON fixtures and the JSON Schema. The Rust tests produce them.
- The private repo also holds `services/` and `infra/`. They are not MIT and stay out of the public
  repo. Never copy their code into an MIT folder.

## Gates

Every change passes these gates before you report it as done.

| Part | Commands |
|---|---|
| One Rust crate | `cargo fmt -p <crate> -- --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>` |
| Workspace | `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` |
| Phone app | `flutter analyze`, `flutter test` (run both in `apps/mobile`) |
| Licenses | `cargo deny check` (when `cargo-deny` is installed) |

- Fix every warning.
- The workspace lints deny `unsafe_code`. Do not add `unsafe` code.
- Run the gates of the crate that you changed. Run the workspace gates when you change
  `aivory-protocol`, because the desktop app and the daemon use it.

## Rules for the desktop build

- `cargo test` and `cargo build` skip the desktop app. The build takes minutes.
  Use `-p aivory-desktop` or `--workspace` to build it.
- Parallel build rule: run a copy of the app, not `target\debug\aivory.exe`. Copy the file to a
  scratch folder, for example `<scratchpad>\<name>\aivory-<name>.exe`. Then another build can
  replace `target\debug\aivory.exe`.
- A screen is done when its screenshot matches the artboard at 1440x900 and all controls call
  the daemon. Use `apps/desktop/tools/window-shot.ps1 -Out <png>` for the screenshot.

## Rules for serde_json

GPUI turns on the `preserve_order` feature of `serde_json`. A workspace build changes the key
order of `serde_json::Value`. A crate build alone does not.

- Do not write a test that depends on the key order of a JSON object.
- Sort the keys before you compare (see `crates/aivory-protocol/tests/fixtures.rs`).

## Protocol and fixtures

- The data types and the JSON-RPC methods are in `crates/aivory-protocol`. Change them there first.
- Regenerate the fixtures and the schema with `AIVORY_BLESS=1 cargo test -p aivory-protocol`.
- The phone tests keep copies of some fixtures in `apps/mobile/test/fixtures`. The shipped phone app has no demo data.
  Copy the changed files after you regenerate them.

## Original code and licenses

- All code is original. Do not copy code from Orca or from any other product.
- Dependency licenses: MIT, Apache-2.0, BSD, ISC, Zlib, Unicode. `deny.toml` enforces this list.
  Do not add a GPL or AGPL dependency.
- Zed editor crates are GPL. Use only the Apache-2.0 GPUI crates (ADR 0020).
- Fonts and icons need a license file next to them.
- Code from the owner's own repos (`flexible-ai`, `aivory-guard`, `aivory_flutter`) may be
  reused. Port it, and keep the license rules above.

## Privacy and native stores

- The native stores (`~/.claude`, `~/.codex`, `~/.gemini`) are read-only. Never write to them.
- Run only the `claude`, `codex`, and `gemini` binaries that the user has installed and logged in.
- Never read, store, or proxy a login token. Do not use an Agent SDK with a subscription login.
- Never print the message content of a real session in a log, a test, or a report.
- Resume only throwaway sessions that you create yourself in a scratch folder.

## Tests

- Use synthetic fixtures. Do not use a real session in a test.
- A test daemon sets `AIVORY_CACHE_DIR` to a scratch folder. This keeps test data out of the
  real store.

## UI rules

- The UI matches `docs/design/screens/` exactly.
- Light theme first. Session panes have square corners. All other UI keeps the radii 4, 8, 12,
  and 18 px, and pills.
- Use color for state only. Show details on hover. Keep the screen calm.
- Use the AIVory logo from `../aivory-hugo/static/images/aivory.svg`.

## Git

- Commit or push only when the user asks.
- Create a branch first. Do not commit to `main`.

## Reports

- Write reports to the user in ASD-STE100 (Simplified Technical English).
  Use short sentences, the active voice, and one word for one meaning.
- Code, commit messages, file contents, and command output stay as they are.
- Give a short answer. Do not add background that the user did not ask for.
- After a step, update `nextsession.txt`: tick the item, add the gate result, and set NEXT STEP.
