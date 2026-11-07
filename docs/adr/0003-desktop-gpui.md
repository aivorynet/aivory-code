# ADR 0003 — Desktop UI on GPUI

Status: Accepted (user decision), 2026-10-21. The spike gate passed on 2026-10-21 (see Amended).
The crate choice is refined by [ADR 0020](0020-gpui-crates.md).

## Context
The desktop shows many live session panes, terminals, diffs, and a code lens with hover.
Start-up time and memory are key requirements.

## Decision
GPUI with gpui-component. We use no Zed crate other than `gpui`, because the Zed editor crates
are GPL. Our lens widget is our own code on gpui-component. Multi-window and tiling are native.
CI runs cargo-deny with a license allowlist.

Spike gate (planned for week 6): 16 live terminal panes at 120 fps, the lens with hover on a 10,000-line
file, and a side-by-side diff. If the gate fails, we change to the fallback.

## Rejected alternatives
- **Tauri 2 + CodeMirror 6 (fallback):** mature editor, but WebView memory and slower start.
- **Flutter on desktop:** one UI codebase, but 0.4-0.8 s start and no proven code editor.
- **Electron:** the heaviest runtime; contradicts the budgets.

## Consequences
- GPUI is before 1.0. We pin a revision and keep the UI thin over the protocol.
- We must verify the gpui-component license (Apache-2.0) before the first release.

## Amended 2026-10-21 (first version)
- The spike gate passed with a changed target. The war room shows 40 live panes at 64-68 fps
  (longest frame 19 ms, 109 MB RAM) in a release build. The target is now 40 panes at 60 fps.
- The code lens with hover and the side-by-side diff are not built. They are not part of the
  first version. The Tauri fallback is not needed now.
- The app uses `gpui-pre`, `gpui-pre-platform`, and `gpui-base`. It does not use
  `gpui-component` (ADR 0020). All three crates are Apache-2.0.
