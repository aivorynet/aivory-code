# ADR 0020 — GPUI crates: gpui-pre, gpui-pre-platform, and gpui-base

Status: Accepted, 2026-10-21. Refines ADR 0003.

## Context
ADR 0003 chose GPUI with gpui-component for the desktop app. The first window (spike on
Windows, 2026-10-21) showed which crates fit together:
- `gpui` 0.2.2 on crates.io is an older snapshot of the GPUI crate of Zed (Apache-2.0).
- `gpui-component` 0.8.0 and `gpui-base` 0.8.0 (Apache-2.0, repo longbridge/gpui-kit) do not
  depend on `gpui` 0.2.2. They depend on `gpui-pre` 0.3.9, a snapshot of zed@93d7456 (Apache-2.0).
- One app can use only one copy of GPUI. Two versions give two sets of types that do not fit.
- In `gpui-pre`, the app entry is in a third crate, `gpui-pre-platform`. The call
  `Application::new()` does not exist.
- `gpui-base` holds the behavior part: text input state and interaction. It has no theme.

## Decision
`apps/desktop/Cargo.toml` uses these three crates:

```toml
gpui = { package = "gpui-pre", version = "0.3.9" }
gpui_platform = { package = "gpui-pre-platform", version = "0.3.9" }
gpui-base = "0.8.0"
```

- The app starts with `gpui_platform::application().run(...)`.
- The app uses `gpui-base` for text input state (`TextareaState`). All other widgets are our
  own code on GPUI, because the accepted design sets its own tokens and shapes.
- The app does not depend on `gpui-component`.
- No Zed editor crate is used. Those crates are GPL.

## Rejected alternatives
- **`gpui` 0.2.2 from crates.io:** an older snapshot. `gpui-base` needs `gpui-pre`, so the app
  would link two GPUI versions.
- **GPUI from the Zed git repository:** `deny.toml` denies unknown git sources. The Zed workspace
  also holds GPL crates; only the GPUI crates are Apache-2.0.
- **The widgets of `gpui-component`:** they bring their own look. The design needs exact
  visuals, for example square corners on session panes.
- **Tauri 2 with the HTML design:** this stays the fallback of ADR 0003. The war room passed
  the performance gate (P3b: 40 panes at 64-68 fps, 109 MB RAM), so we do not use it now.

## Consequences
- `gpui-pre` is a snapshot, not a release line. Pin the exact version `0.3.9`. Update the three
  crates together and test the screens after each update.
- Examples for `gpui` 0.2.2 do not always apply. Check the API in the `gpui-pre` source.
- A debug build of the desktop app takes minutes (first build 3 min 46 s, rebuild 2 min 36 s).
  For this reason the app is not a default member of the workspace.
- All three crates are Apache-2.0, and `deny.toml` allows this license. `cargo-deny` is not
  installed on the main machine yet. Run `cargo deny check` before the first release.
- The risk of ADR 0003 stays: GPUI is before 1.0, and the UI stays thin over the protocol.
