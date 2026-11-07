# Release pipeline (GitHub Actions)

## Make a release
1. Set `version` in the root `Cargo.toml` (`[workspace.package]`).
2. Tag and push: `git tag v0.1.0 && git push github v0.1.0`. A tag `v*` starts `release.yml`.
3. Run `release.yml` by hand (workflow_dispatch) for a test build. It builds but makes no release.

`ci.yml` runs on each push to `main` and on each PR: fmt, clippy `-D warnings`, tests, `cargo deny`.

## Artifacts
| OS | Files |
|---|---|
| Windows x64 | `aivory-windows-x64.zip`, NSIS `*-setup.exe` |
| macOS arm64, x64 | `*.dmg` (unsigned), tar.gz |
| Linux x64 | `.deb`, `.AppImage`, tar.gz |

Each package holds `aivory` (desktop app) and `aivoryd` (daemon) side by side.
Packaging uses `cargo-packager` (MIT/Apache-2.0). Config: `[package.metadata.packager]` in `apps/desktop/Cargo.toml`.

## Secrets (all optional)
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` (macOS sign and notarize),
`PACKAGER_SIGN_PRIVATE_KEY` (updater signature). Windows code signing is not wired.

## Known gaps
- Icons: not made. Create `apps/desktop/packaging/icons/` from `../aivory-hugo/static/images/aivory.svg`, then uncomment `icons`.
- `license-file` points to `LICENSE` in the repo root. Add it if missing.
- No service registration. The installer only places `aivoryd` next to `aivory`. The app starts it (verify). A Windows service, launchd agent, or systemd unit is future work.
- macOS builds are unsigned: users must right-click Open. Not notarized.
- `cargo-packager` flags and the `macos-13` runner label need a first real run to confirm.
- Linux ARM, Windows ARM: not built. Flutter phone app: not in this pipeline (add an Android APK job later; skip iOS).
- Private folders are never packaged.
