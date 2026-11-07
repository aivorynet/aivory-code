# ADR 0022 — One theme system for the whole desktop app

Status: Accepted, 2026-10-30 (owner OK). Replaces the war-room-only skins of
2026-10-29 (`Display.skin`, `warroom/skins/*`).

## Context
The desktop app has five looks: Minimal, Bridge, Operator, Neon (`Neon`) and Navigator.
Today they are war-room skins only. The owner did not accept them (verdict 2026-10-30):

- A look changes only the war room. The shell, Inbox, Tasks, Repos, Settings, the Focus view
  and the terminal panes stay Minimal.
- The look switch is in the war-room header only.
- Each skin reads `WarRoom` in its own way and calls the daemon in its own way
  (`skins/common.rs` shares only a part of it). A new element must be built five times.
- The layouts do not work for 1 session and for 30 sessions.
- Operator mixes items that the app computes (radar, gauges, trace) with the real sessions.

The style tokens are `const` values in `theme.rs`. 41 files use them, with 282 direct
`theme::X` uses. A const cannot change at run time.

## Decision
A look is an app-wide theme. Each theme draws every element of the app from one read model,
and changes data through one action API only. Minimal is the reference implementation.

### 1. Names and settings
- `Display.skin: WarRoomSkin` becomes `Display.look: Look` in `aivory-protocol`
  (`Minimal`, `Bridge`, `Operator`, `Neon`, `Navigator`). Serde reads the old `skin` key and
  the old value `neon` as aliases, so old settings files stay valid.
- `Display.theme` (Light, Dark, System) stays. It applies to Minimal only. The other looks
  have one fixed palette each.
- The user sets the look in two places, and both write `settings/set`:
  1. Settings > Display: one card per look, with a small preview.
  2. The app frame: a look button in the status bar, on every screen. `Ctrl+Shift+L` opens it.
- The war-room header has no skin menu after this change.

### 2. The read model
A new module `apps/desktop/src/model/` holds one GPUI entity `Model`. The daemon client is the
only writer. Every screen and every theme reads `model.snapshot()`.

| Part | Content | Source |
|---|---|---|
| `codebases` | id, name, parts (name, path, live and active flags) | `WarRoom.codebases` |
| `sessions` | id, title, codebase, part, provider, model, state, gate, since, cost, tokens, last tool, open ask, mode | `WarRoom.sessions` |
| `asks` | the ask queue, the longest wait first | `decisions()` |
| `gates` | counts and ids per gate: Needs you, Working, Blocked, Idle | `sessions` |
| `usage` | windows, cost, tokens, activity samples | `WarRoom.usage`, `activity` |
| `events` | a ring of the last 200 events (state change, ask, tool, error) | `warroom/delta` |
| `inbox`, `tasks`, `repos` | the lists of the screens | `inbox/list`, `tasks/list`, `repos/list` |
| `daemon` | link state, autopilot, queue length, version | `daemon/info`, `autopilot/get` |
| `analysis` | items that the app computes, each with `Analysis { source, label }` | the model |
| `intent` | the current focus mode (section 5) | the model |

Rules:
- An item in `analysis` is never a session. A theme draws analysis items in an own zone with
  the label "Analysis". Radar, gauges, trace, trend charts and forecasts are analysis items.
- The model computes all counts, orders, and text formats (`thousands`, `rate_text`, `clip`,
  short tool names). A theme does not compute them again. The helpers of `skins/common.rs`
  move to `model/`.
- The model has no GPUI element code. Unit tests cover it with synthetic fixtures.

### 3. The action API
`model/actions.rs` has one enum and one function. No theme and no screen calls `rpc` directly.

```rust
pub enum Action {
    Answer { id: String, option: Option<String>, text: Option<String> }, // session/answer, inbox/answer
    Focus { id: String },                                               // opens the Focus view
    Dispatch { codebase: String, part: String, prompt: String },        // tasks/create or session/new
    NewSession(NewSessionParams),                                       // session/new
    Send { id: String, text: String, composer: Option<ComposerPrefs> }, // session/send
    Pause { id: String }, Park { id: String }, Stop { id: String }, Resume { id: String },
    Autopilot(Autopilot),                                               // autopilot/set
    PermissionMode { id: String, mode: Mode },                          // session/send composer
    Open(Screen),                                                       // shell navigation
    SetLook(Look),                                                      // settings/set
}
pub fn act(action: Action, window: &mut Window, cx: &mut App);
```

- `PermissionMode` has no own RPC method. It applies from the next turn, through the
  `composer` of `session/send`. The UI says "from the next turn".
- `act` shows a toast on an RPC error. All themes show the same error text.

### 4. The component contract
`apps/desktop/src/look/` defines the contract. Each element has a props struct with data and
state only, and one method on the trait `LookKit`:

| Element | Props (short) | Used by |
|---|---|---|
| `app_frame` | screen, nav, status bar, body | shell |
| `nav` | screens, current screen, badges | shell |
| `status_bar` | daemon link, live count, gates, cost, look button | shell |
| `list_row` | title, meta, state, gate, actions, selected | Inbox, Tasks, Repos, war room 13+ sessions |
| `card` | title, meta, state, gate, last tool, ask, actions | war room 1..12 sessions, Overview |
| `ask` | ask kind, question, options, keys | war room, Focus view, Inbox |
| `chart` | series, axis, unit, analysis flag | usage, Overview |
| `gauge` | value, max, unit, label, analysis flag | status, Overview |
| `editor` | file, diff, hunks, caret | Focus view |
| `terminal` | pane lines, composer, `PaneTheme` | session panes, Focus chat |
| `dialog` | title, body, buttons | all dialogs and popups |

```rust
pub trait LookKit {
    fn tokens(&self) -> &Tokens;                  // colors, fonts, sizes, radii, motion
    fn app_frame(&self, p: FrameProps, cx: &mut App) -> AnyElement;
    // ... one method per element in the table ...
    fn scene(&self, screen: Screen) -> Option<&dyn Scene> { None }
}
```

- `Minimal` implements every method. The trait methods of the other looks default to the
  Minimal method with their own `Tokens`. Thus a new look covers the whole app from the first
  day, and replaces elements one by one.
- `Tokens` replaces the `const` values of `theme.rs`. It is a GPUI global; the look switch
  replaces it and refreshes all windows. The `const` values stay as the Minimal light tokens
  during the change, so each file can move in its own commit.
- A `Scene` is an optional full-screen view for one screen (for example the Navigator 3D
  landscape or the Neon map in the war room). A scene reads the same model, calls the
  same actions, and draws asks with the `ask` element of its look. A scene must also have a
  list mode (section 6) for many sessions.
- A theme does not add own data. If a look needs a value, the value goes into the model first.

### 5. Intent modes
The model sets `intent` from the data. All looks get it; each look decides how it shows it.
Bridge changes its layout for each mode, with eased transitions (250-400 ms).

| Intent | When | What the screen puts first |
|---|---|---|
| `Alert` | an ask is open, or a session has an error | the ask queue, one key per option |
| `One` | the user selects one session | that session: chat, last tools, diff |
| `Ops` | the user dispatches or changes autopilot | dispatch, queue, autopilot, modes |
| `Monitor` | sessions run, no ask | all sessions, live activity |
| `Analysis` | the user opens the analysis zone | usage, charts, analysis items |

The user can pin a mode. An ask overrides a pinned mode only with a notice, not with a jump.

### 6. Responsive layout rules
Width classes, from the window width:

| Class | Width | Layout |
|---|---|---|
| `Compact` | < 960 px | nav as icons, no right sidebar, the sidebar opens as an overlay |
| `Regular` | 960-1439 px | nav as icons, right sidebar 320 px, can close |
| `Wide` | >= 1440 px | nav with labels, right sidebar 360 px |

Density tiers, from the number of live sessions in the war room:

| Sessions | Layout |
|---|---|
| 1 | one large card with the live tail and the ask |
| 2-4 | large cards in one row or a 2x2 grid |
| 5-12 | compact cards in a grid, the column count from the width |
| 13-30 | list rows, grouped by codebase |
| 31+ | list rows, groups closed except the groups with an ask or an error |

- Sessions with an ask stay at the top in every tier.
- Text never clips without an ellipsis and a hover with the full text.
- Each look keeps these tiers. A scene uses its list mode from 13 sessions.
- The gates (Needs you, Working, Blocked, Idle) are filters with counts in all tiers.

### 7. Quality gates per look
- Screenshots at 1440x900 and 960x640, each with 1, 6 and 30 sessions (`--demo --demo-rich`
  and a new `AIVORY_DEMO_SESSIONS=<n>`).
- Every control calls `act`. A test checks that each `Action` maps to one RPC method.
- A release build draws a frame in less than 16 ms with 30 sessions.
- Real-data screenshots never leave the machine.

## Order of work
1. `model/` and `actions` with tests; the war room and the skins read them.
2. `Tokens` global and `LookKit`; Minimal as the reference: shell, war room (new gates, cards,
   right sidebar, Open in Focus view, module lines with meaning and active state), Inbox,
   Tasks, Repos, Settings, Focus view, panes. The look switch in Settings and the status bar.
3. One look at a time, owner check after each: Bridge, Operator, Neon, Navigator.

## Rejected alternatives
- **Tokens only (colors and fonts):** Bridge and Navigator need other layouts, not other colors.
- **One full app per look:** five copies of each screen. Each new feature costs five builds.
- **Looks as data files (JSON):** GPUI layouts are code. A file format adds a parser and no
  benefit for five built-in looks.

## Consequences
- 41 files change from `theme::X` to tokens. The change is mechanical and goes file by file.
- The war-room skin code moves into `look/<name>/`; the shared code moves into `model/`.
- The protocol changes (`Display.look`). The fixtures, the schema and the phone copy of
  `demo-settings.json` need a new bless.
- The phone app keeps one look. It reads `look` and ignores it for now.
