# ADR 0023 — A slim background service, and displays on every device

Status: Proposed, 2026-11-03. The owner chose the direction (answers below); the text needs
the owner's OK. Builds start after the look builds of `docs/design/looks/`.

## Context
Today the desktop app is the product. `aivoryd` reads the native session stores, runs the agent
CLIs and serves JSON-RPC, but it runs only when someone starts it, and the desktop app shows
"waits for aivoryd" without it. The phone app (Flutter, ADR 0004) is a small client over the
LAN link (ADR 0021); iroh remote and push are planned (ADR 0015).

The owner wants (2026-11-03):
- A slim background service that people launch once, that starts with the system, and that
  wraps all sessions.
- The displays we design (the looks) on an iPad, a phone or a desktop. The desktop app still
  works.
- Mobile apps made by a build agent: the iPad looks like the desktop app; phones look different.
  A foldable iPhone and an Apple Watch companion are included.
- Flutter first, to test and tune; then native apps for iOS and Android.

## Decision

### 1. The service is the product core
- `aivoryd` runs as the user, not as a system service: the agent CLIs, their logins and the
  session folders belong to the user.
- It starts at login and restarts after a crash:
  - Windows: a per-user Task Scheduler task "at logon" (restart on failure).
  - macOS: a LaunchAgent with `KeepAlive`.
  - Linux: a `systemd --user` unit.
- One instance per user (a lock next to the pipe). `aivoryd install` and `aivoryd uninstall`
  write and remove the login item.
- Closing a display never stops the service.

### 2. A tray companion
- A small binary `aivory-tray` (Rust, MIT/Apache crates only) beside `aivoryd`. It holds no
  state; it is a client of the service.
- The icon shows the state: idle, working, waits for you, error.
- The menu: the waiting decisions (a permission answers with Allow / Deny right there; a plan or
  a question opens a display), Open display, Pause all, Autopilot on/off, Look, Quit display.
- A native notification for each new decision.

### 3. Displays are clients of one protocol
| Display | Technology | Layout |
|---|---|---|
| Desktop | GPUI app (`apps/desktop`), unchanged | The looks of `docs/design/looks/` |
| iPad | Flutter first, native later | The desktop layouts and looks |
| Phone | Flutter first, native later | Own phone layouts (design round first) |
| Foldable iPhone | Same phone app | Folded: phone layout. Unfolded: two panes (list + session) |
| Apple Watch | Native watchOS from the start (Flutter has no watchOS) | Waits list, Allow / Deny, a short reply by dictation |

- Transport: the local pipe on the same computer; the LAN WebSocket link (ADR 0021) and iroh
  (ADR 0015) for other devices. The same JSON-RPC methods on every transport.
- The read model and the action API of ADR 0022 move into `aivory-protocol` as a snapshot
  method plus the actions (answer, focus, dispatch, park, stop, resume, autopilot, permission
  mode). Every display renders the same snapshot; no display has private logic.

### 4. Flutter first, then native
1. Flutter (`apps/mobile`) carries the iPad looks and the new phone layouts. We test and tune
   them there.
2. After the owner accepts the Flutter apps, a build agent rebuilds them natively: SwiftUI for
   iPad, iPhone and the Watch; Kotlin with Jetpack Compose for Android. This replaces
   ADR 0004 (pure Dart) for the shipped apps; Flutter stays as the reference.

## Order
1. S1: login item, single instance, `aivoryd install`, tray companion, notifications.
2. S2: snapshot + actions in the protocol; displays use only these.
3. S3: phone design round (owner), then Flutter: iPad looks, phone layouts, foldable two-pane.
4. S4: Apple Watch companion (native).
5. S5: native iOS and Android rebuild after the owner accepts S3.

## Consequences
- The desktop app becomes optional. It starts `aivoryd` when the service is not running.
- Every new screen element lands in the protocol snapshot first, then in each display.
- App store signing, TestFlight and Play accounts are needed for S5 (owner).

## Open questions
- Phone layouts per look: does each look get a phone version, or does the phone use one look?
- Quick answers from the tray and the Watch: which decision types (permission only, or plans
  and questions too)?
