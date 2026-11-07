# Contributing to AIVory Code

Thanks for helping! Bugs, ideas, looks and code are all welcome.

## Issues and feature requests

- **Bug:** what you did, what you expected, what happened. Add a screenshot and your OS.
- **Feature:** what you want to do and why. A sketch helps.
- Never paste the content of a real agent session (it can hold private code or secrets).

## Pull requests

1. Fork the repo and create a branch.
2. Read [AGENTS.md](AGENTS.md): it lists the gates and the rules (they apply to people too).
3. Run the gates of the part that you changed, for example:
   `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`; for the phone app `flutter analyze` and `flutter test`.
4. Keep a PR small and focused. Add a screenshot for UI changes.
5. All code must be original. Dependencies must use MIT, Apache-2.0, BSD, ISC, Zlib or Unicode
   licenses (no GPL or AGPL).

## Looks (themes)

See "Write your own look" in the [README](README.md#write-your-own-look). Fonts, icons and
sprites need a license file next to them. Use original art, and do not copy the art or the
name of a film, a show, or a game.

## License

By contributing you agree that your work is licensed under the MIT license
([LICENSE](LICENSE)).
