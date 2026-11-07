# apps/desktop

The AIVory Code desktop app. It is a GPUI client of `aivoryd`. The crate is `aivory-desktop`
and the binary is `aivory`. The design is in `docs/design/screens/README.md` (ADR 0003, ADR 0020).

Status: War room, Session, and Inbox are done. Tasks, Repos, and Settings are open
(see `docs/roadmap.md`).

```
cargo run -p aivory-desktop -- --demo
cargo run -p aivory-desktop -- --help
```

- `--demo` serves the demo data in this process. Without it, the app connects to a running
  `aivoryd` (`--endpoint <name>`).
- `--stress <N> --fps` is the performance check.
- The first build takes several minutes. Run a copy of the binary when you build in parallel
  (see `AGENTS.md`).
- Fonts are in `assets/fonts` (SIL Open Font License). `tools/window-shot.ps1` takes a screenshot.
