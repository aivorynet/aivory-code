# AIVory Code

AIVory Code is a local-first control room for coding agents. It shows the sessions of Claude Code,
Codex CLI, and Gemini CLI on one screen. You see which agent works, which agent waits for an
answer, and which agent has an error. You answer from the desktop app or from your phone.

A Rust daemon (`aivoryd`) reads the native session stores and runs the agent CLIs that you
already have. The desktop app and the phone app are clients of this daemon.

Status: first public version. See [docs/roadmap.md](docs/roadmap.md).
Windows 11 is the supported platform. The macOS and Linux builds are a preview: they build in CI,
but nobody has tested them yet. The Windows build has no code signature yet, so SmartScreen
shows a warning at the first start.

<p align="center">
  AIVory Code is free and open source. If it saves you time, I'd love a <a href="https://buy.stripe.com/5kQbJ3csm21Xfz64DhdjO00">coffee</a> or a <a href="https://github.com/aivorynet/aivory-code">star</a>.<br>
  Everybody is welcome: <a href="https://github.com/aivorynet/aivory-code/issues/new?labels=bug">report a bug</a>, <a href="https://github.com/aivorynet/aivory-code/issues/new?labels=feature">ask for a feature</a>, <a href="https://github.com/aivorynet/aivory-code/pulls">open a pull request</a>, or <a href="#write-your-own-look">write your own look</a>.
</p>

## Join in

This project grows with the people who use it. You don't need permission to help:

- **Found a bug?** [Open an issue](https://github.com/aivorynet/aivory-code/issues/new?labels=bug). A screenshot and the steps to repeat it are perfect.
- **Missing something?** [Ask for a feature](https://github.com/aivorynet/aivory-code/issues/new?labels=feature). Tell us what you want to do, not only how.
- **Want to code?** Pick an issue, or bring your own idea, and [open a pull request](https://github.com/aivorynet/aivory-code/pulls). Small PRs are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).
- **Like it?** A [coffee](https://buy.stripe.com/5kQbJ3csm21Xfz64DhdjO00), a star or a short review helps a lot.

### Write your own look

The app has looks (themes) that change the whole screen: Minimal, Bridge, Operator, Neon,
Navigator, Holo, Tactical and Command (a pixel-art building game where your agents construct your
features). Make your own and share it:

- A theme is a GitHub repo with plain data files: colors, fonts, and screen layouts. No code runs.
  Start with `aivoryd theme init my-theme`, and check it with `aivoryd theme check my-theme`.
- Install one in Settings > Display: paste the GitHub link, confirm, select. Or run
  `aivoryd theme add github:<owner>/<repo>`.
- The format is in [ADR 0024](docs/adr/0024-theme-packages.md). Your feedback is welcome.
- Use original art, or art with a free license, and put the license file next to it.
- Add it to the [theme registry](https://github.com/aivorynet/aivory-themes) with a pull request.

## Parts

| Folder | What | Language |
|---|---|---|
| `crates/aivory-protocol` | The contract between daemon and clients: data types, JSON-RPC methods, layout helpers, demo data | Rust |
| `crates/aivory-sessions` | Read-only index of the native session stores; transcripts, session state, spans | Rust |
| `crates/aivory-agents` | Detects the agent CLIs and runs them; maps their output to events | Rust |
| `crates/aivory-map` | Reads and writes the part map of a codebase (the file `.aivory/map.toml` in that codebase) | Rust |
| `crates/aivory-engine` | SQLite store, activity sampler, usage estimate, autopilot planner | Rust |
| `crates/aivory-syntax` | Syntax layer: tree-sitter parse, highlight, stubs, outline, syntax errors (19 languages) | Rust |
| `crates/aivory-index` | Symbol index per repo: stub store, segments, session deltas, definition and references | Rust |
| `crates/aivory-diag` | Diagnostics: rule packs, linters, LSP pool, SARIF per turn | Rust |
| `crates/aivory-vcs` | `Vcs` trait and Git backend, turn snapshots, turn ledger, blame by prompt | Rust |
| `crates/aivory-coord` | Local coordination: claims, conflict radar, mailbox | Rust |
| `crates/aivory-link` | Remote link to the phone: iroh endpoint, device keys, sealed push payloads | Rust |
| `crates/aivoryd` | The daemon and its command line | Rust |
| `apps/desktop` | The desktop app (GPUI), binary `aivory` | Rust |
| `apps/mobile` | The phone app | Dart (Flutter) |
| `proto/` | JSON fixtures and the JSON Schema of the protocol | JSON |
| `docs/` | Design, decision records (ADR), roadmap | Markdown |

These folders hold only a README. They are placeholders for later work: `grammars/`,
`packs/`, `templates/`, `plugins/genie/`.

## Build and run

Install Rust 1.88 or newer. Install Flutter for the phone app. The first desktop build takes
several minutes.

Run the desktop app on the demo data. No agent and no daemon are needed:

```
cargo run -p aivory-desktop -- --demo
```

Run the desktop app on the real sessions of this machine. Use two terminals:

```
cargo run -p aivoryd -- serve
cargo run -p aivory-desktop
```

Add `--scope <folder>` to `serve` to show only the sessions below one folder.

Run the phone app. It uses the demo data. The link to the daemon is not finished yet.

```
cd apps/mobile
flutter run -d windows
```

Run the stress check (40 live panes, frame rate in the log):

```
cargo run --release -p aivory-desktop -- --demo --stress 40 --fps
```

Use the daemon command line without the apps:

```
cargo run -p aivoryd -- --help
cargo run -p aivoryd -- list
cargo run -p aivoryd -- map init <repo>
```

## Test

```
cargo test
cargo test --workspace
cd apps/mobile && flutter test
```

- `cargo test` tests the crates. It leaves out the desktop app.
- `cargo test --workspace` adds the desktop app. The build is slow.

Run these checks before you finish a change:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cd apps/mobile && flutter analyze
```

`cargo deny check` checks the dependency licenses. It needs `cargo install cargo-deny`.

## Privacy rules

- The native stores (`~/.claude`, `~/.codex`, `~/.gemini`) are read-only. The code never
  writes to them.
- The daemon runs `claude`, `codex`, and `gemini` with the login of the user.
- No code reads, stores, or proxies a login token.
- The daemon keeps its own data (tasks, settings, devices) in a per-user folder.
  On Windows this is `%LOCALAPPDATA%\aivory\code`.
- Tests use synthetic fixtures. No test reads a real session.
- The phone link works in the local network only. It has no TLS yet
  ([ADR 0021](docs/adr/0021-lan-websocket-link.md)).

## Documents

- [AGENTS.md](AGENTS.md): rules for coding agents and contributors.
- [docs/design/screens/README.md](docs/design/screens/README.md): the accepted design.
- [docs/design/00-vision.md](docs/design/00-vision.md): vision, principles, and glossary.
- [docs/adr/](docs/adr/): decision records.
- [docs/roadmap.md](docs/roadmap.md): steps and status.

## License

MIT. Everyone can use, change, and share AIVory Code for free, also at work. See [LICENSE](LICENSE).
There is no paid tier ([ADR 0025](docs/adr/0025-free-for-everyone.md)).
The bundled fonts use the SIL Open Font License. The license files are in
`apps/desktop/assets/fonts` and `apps/mobile/assets/fonts`.
