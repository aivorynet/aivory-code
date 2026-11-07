# Accepted design — screen inventory

Source of truth: the Design canvas https://claude.ai/artifact/SqKd3KhDJPTX1Sr1vkCUkM
(accepted 2026-10-21). The `.dc.html` files in this folder are copies of its artboards.
They need the canvas runtime (`support.js`) to render, so read them as a spec: markup
for the layout, the `<helmet><style>` block for exact tokens, and the
`class Component extends DCLogic` block for sample data and behavior.

This inventory replaces the layout parts of `07-ux-desktop.md` and `08-ux-mobile.md`
and closes `war-room-brief.md`.

## Shared tokens (all screens)

| Token | Value |
|---|---|
| Fonts (rule of 2026-10-23, wins over the artboards) | IBM Plex Sans for all interface text, names included (codebases, parts, agents, models, sessions). JetBrains Mono only for code, commands, file paths and trees, symbols, diff line numbers, and numbers that line up in columns. Console fonts: JetBrains Mono, IBM Plex Mono, Fira Code, Source Code Pro, Roboto Mono |
| Type scale (desktop) | Only five sizes, px at 100 %: caption 12 (meta, hints, counts, status bar), small 13 (chips, pickers, buttons, table cells, labels), body 14 (text, prose, inputs), title 16 (card, pane and section titles), heading 20 (screen titles). Settings → Display → Interface text size scales them (80-150 %, default 115 %). The console text uses the console font size (default 16 px); prompts and agent prose use body. |
| Ink | #1A1A2E primary; #4A5568, #5A6578 secondary; #8A93A6 muted |
| Surfaces | page #F6F7F9; card #FFFFFF; track #F1F3F6 / #ECEEF2; selected #EEF3FE; line rgba(13,13,14,.08) |
| Actions | primary #3574F0 (hover #2C63D2); link #4B5BDB; dark button #0D0D0E |
| Rainbow strip (3 px, top) | #F59E0B, #F97316, #EF4444, #7C3AED, #6B8AFF, #51B5BF, #059669 |
| State `w` working | ink dot #1A1A2E; in-progress segment #6B8AFF |
| State `q` needs you | #F97316 (text #C2410C, bg #FFF1E6) |
| State `e` error | #DC2626 (text #B91C1C, bg #FDECEC) |
| State `i` idle | hollow ring #8A96A8; chart fill #CBD2DC |
| State `p` parked, `d` done | #A0AEC0; done/ok/review green #16A34A (text #15803D, bg #EAF7EE) |
| Subagent, conflict | #7C3AED (text #6D28D9, bg #F3EEFF) |
| Providers | claude #D97757, codex #1A1A2E, gemini #4B5BDB, opencode #7C3AED, goose #0D9488 |
| Shapes | cards radius 12, buttons 6–8, pills 999. Session panes: square corners |

Console backgrounds (pane theme): White #FFFFFF, Grey #F4F5F7, Dark #15161B (text #E4E5EA),
Navy #0D1624 (text #E4E7F0). The dark variables are in `Main.dc.html` (`.grid.th-dark`).

One status model for all screens: `q` needs you (kind: permission, plan, question),
`e` error, `w` working, `i` idle, `p` parked (Tasks: planned), `d` done.

## War room v2 and the Scipio look, 2026-10-27 (wins over every war room rule below)

Canvas https://claude.ai/artifact/SqKd3KhDJPTX1Sr1vkCUkM, artboard `WarRoomV2.dc.html`
("original + focus panel, code, bulk actions"). The look moves toward the Scipio ERP app.

- Look: two surfaces only. Page white; the rail and the right column are white with a soft
  shadow, no region borders. Everything you act on is a white card with one soft shadow
  (`0 0 0 1px #0000000A, 0 2 6 #0000000D, 0 10 28 #0000000D`), no border. Ink graphite #16171B
  (#3A3A3F, #6E6E73), soft grey #F2F2F7 (hover #E5E5EA) for secondary buttons, fields, chips
  and the active rail item. Main action graphite. Crimson #D20025 only for "needs you", amber
  #9A5200 for an error. Switch on = #34C759. No rainbow strip.
- Shapes: buttons are pills (36 px, small 32 px, weight 600), segments are pills, decision
  tiles 24 px, session panes 18 px, composer 14 px, popup 28 px.
- Type: the title face of the Scipio app (`../scipio-ai`, aurora theme): Bricolage Grotesque,
  weight 600, from its weight-only woff2 (static TTF cut, OFL) for the screen title (26 px)
  and the big numbers; IBM Plex Sans for the rest; JetBrains Mono for paths and meta.
- Header 72 px: title, search pill, Autopilot, Overview toggle, New session.
- Page: centered, max 1240 px, 56 px side padding. Sections: **Decisions** (one tile per open
  ask, the longest wait first, two answers + Context; "Answer one by one" walks them in the
  focus panel), **Sessions** (filter All / Needs you / Working / Idle with counts, the Sankey
  filter chip, panes per row Auto + 1-16, Display), one group per codebase (no separate
  "Needs you" group), then **Parked and done** pills with Resume.
- Pane: white card, 18 px, light console by default (Display keeps Dark), 14 px console text.
  Calm (owner 2026-10-27, "super distracting"): no state ring, no live status line; the state
  shows in the dot and the wait bar only. Header on two lines: dot (hover: select box), title (click = focus panel), meta (hover:
  pause, stop, deep dive); `codebase / part`, `agent model`, code button, subagents. An ask
  shows as a tinted bar "Permission required · answer in Decisions" (click = context popup).
  Composer: grey field, mode and model only; focus = white with a 2 px ink ring. Idle panes
  read at 60 %. Auto columns = floor((pageW + 24) / (font × 30 + 24)).
- Right column (only at 1100 px and wider; the owner rejected the Overview on 2026-10-27, its
  replacement is open): the Overview (380 px: spend, live agents, agents
  per minute, providers with their usage window, "who works where" Sankeys; a Sankey title
  filters the grid), or the focus panel of one session (42 % of the room, 520-680 px): nav
  "n of m", pause, stop, deep dive, close; tabs Chat and Code (files + diff, "Ask agent",
  "Open in IDE"); the ask card with every answer; the full composer.
- Context popup: the question and the answers, the chat on the left, the first changed file.
- Ticked panes show the bulk bar: Message all, Pause, Park, Stop, Clear.

## One system, accepted 2026-10-23 evening (wins over the tokens and pane rules above)

Canvas https://claude.ai/artifact/AmYQ65NYussLXLX1R8wjdy (`Main.dc.html` Paper, `Graphite.dc.html`,
`System.dc.html`). The layout and the Genie pane structure stay; the look changes.

- One theme for the app AND the panes. Paper (default): page #E9ECF0, card/pane #FFFFFF, pane
  header and ask card #F5F6F8, code #F5F6F8, prompt band #F1F3F6, field #F9FAFB, muted ink
  #646C7E (4.5:1). Graphite: page #0A0B0E, card/pane #1A1B21, header #202128, code #22232A,
  prompt #2A2C33, field #15161B, ink #E6E7EC / #B8BECB / #979EAD. No separate console background.
- Type: caption 12/16, small 13/20, body 14/22 (prose) or 14/20 (UI), heading 20/28; pane title =
  card title = 14/600; code 13/20 (body − 1). Weights 400/500/600 only. Text size S/M/L presets
  with whole pixels (S 12/12/13/12/18, M 12/13/14/13/20, L 13/14/16/15/22 = caption/small/body/
  code/heading); they replace the 80-150 % scale. Fixed line heights everywhere (no clipped
  descenders in the Sankey labels; labels 12 px, parts 500, never bold).
- Panes: no border, square corners; the colour step pane vs page is the edge. Header bar on its
  own tint. No orange or amber borders anywhere.
- Needs you / error: the header bar gets a soft tint (#FDEFE3 / #FCEBEB; Graphite #2E2319 /
  #301E20). Display → Highlight offers Tinted header (default), Lift (only waiting panes white,
  the rest #F4F5F7), None. The ask card: header tint, 1 px line, title in the state text colour.
- Dim: the chat content of a pane is at 50 % unless the pointer is over the pane or focus is in
  it. The ask card, the header and the input never dim.
- Waiting panes: the input box is one line until clicked; the last prompt is sticky at the top of
  the transcript (2 lines max, full text on hover); the transcript follows the newest line.
  Waiting panes are 520 px tall.
- Compact pane under 460 px wide (4 per row at 1440): header on two lines (title, subagents,
  open / `agent · model · part`, wait); input = field on top, one row of wand, clip, mode, model,
  `ctx %`, mic, send (effort, cost and the ctx bar hide); code and diff lines wrap.
- The Sankey cards stay full at all times (a folded strip was rejected).
- Primary actions are blue #3574F0 everywhere (New session too); buttons radius 4, inputs and
  code radius 8, cards 12.

## Desktop shell (all desktop screens)

- 3 px rainbow strip; 76 px left rail: logo, War room, Inbox (orange count badge), Tasks,
  Repos, Settings (bottom).
- 60 px white header per screen; 30 px mono status bar: `aivoryd running`,
  `N running · N parked`, provider window % (claude 5 h, codex, gemini), hub, relay, clock.
- Under 720 px: the rail hides, one column. Under ~1100 px: right detail panels hide.

## 1. War room — `Main.dc.html` (the 96 % screen)

Header: "War room", pills `N need you` and `N error`, Autopilot button (switch + popover),
Search ⌘K, New session.

Autopilot popover: on/off switch; steppers Park idle after (5–120 min, step 5, default 15),
Max running (1–64, default 14), Max per codebase (1–16, default 4); switch
"Start queued tasks · N queued".

Codebases band:
- Pulse chart, last 60 min, one bar per minute: the upper half is active agents (ink),
  the lower half is waiting (orange) plus idle (grey). Hover shows the time and the
  counts in the readout `HH:MM ■ N active ■ N wait for you ■ N idle`.
- One Sankey card per codebase with live sessions, in a 4-column grid (2 under 1100 px,
  1 under 720 px). Columns: agent → part → state. Flow value = 1 + subagent count.
  Link color by state: working ink 9 %, idle #8A96A8 18 %, needs you orange 50 %,
  error red 50 %; attention links draw on top. Labels: `claude ×3`, the part name (bold),
  `needs you 2` (state color). The card title jumps to the codebase group. The layout
  code is `sankey()` in the script block.

Session grid:
- Groups: "Needs you" first (oldest wait first, full row, `where` = codebase / part),
  then one group per codebase (its other sessions; the group spans min(count, cols)).
  Dense auto-flow packs small groups side by side.
- The first group header holds "Panes per row" (Auto · N, 1, 2, 3, 4, 8, 9, 16) and the
  Display popover: font size 9–18 px, console font, background, preview line, reset.
- Auto columns = floor((gridW + 12) / (fontSize × 34 + 12)), max 16. Pane height by
  columns: 1→640, 2→580, 3→520, 4→480, ≤7→420, ≤9→300, else 200 px. Mini mode at 8+
  columns, micro at 16.

**Pane redesign, accepted 2026-10-23 (it wins over the older pane list below):** canvas
https://claude.ai/artifact/43rrLMnYoNJ6vc14DSo1cu, artboard `Main.dc.html` (interactive). One
style only: the Genie chat style (`flexible-ai` `ui/chat/ChatPanel.kt`, `ChatToolbar.kt`,
`PermissionCard.kt`). No avatar icons, no Pause button, no shadow rings or inset-shadow edges.
- Header: one compact row (min 36 px, padding 4×8, 1 px bottom line, `--top` surface): state
  dot, bold title, muted `agent · model · part`, meta (`waits N min` in state color), one
  "open deep dive" icon. Top border 2 px orange only while the session waits for you.
- Transcript: starts at the top and grows down; it scrolls. Message labels `you · 14:31`,
  `claude · 14:33` (mono, muted). Your prompt: grey band, radius 8. Agent text: IBM Plex Sans
  14 / 23. Tool steps fold into one line (`✓ 5 steps · Read · Grep · … ▸`). Diff: box with
  radius 8, file header (draggable, grip ⠿), line numbers, green and red line tints.
- Permission: Genie card IN the chat flow: elevated surface, radius 8, 1 px amber border,
  bold amber title `Permission required: Bash`, mono command, buttons Allow (blue default),
  Always allow, Deny (height 28, radius 4). After a pick the buttons change to the resolved
  text (`Allowed`, `Always allowed`, `Denied`).
- Input box (Genie): radius 12, 1 px border, no shadow. Top row inside the box: media and
  links made in the session (thumbnails 40×28, link pills, count), own lighter background.
  Then: wand (capture) over paperclip (attach) on the left; the text field (own background,
  1 px border, radius 6, 84 px tall, blue border on focus) with the pickers row under it
  (mode, model, effort, then `ctx N %`, a 40 px bar, `$cost` on the right); send icon on the
  right. No separate status bar.
- References: drag or click a thumbnail, a link, or a diff header into the text field to add
  an `@reference` chip. The daemon parses `@references` and URLs in a prompt and offers the
  agent a tool to open each one (as Genie does).
- Console themes Dark (default), Navy, Paper, Grey: the CSS variables are in the artboard.

Older pane spec (terminal look, square corners, 1 px border, 2 px inset top edge: orange for
`q`, red for `e`):
- Line 1: state dot, title, meta (`waits N min`, `idle N min`, `N min`). On hover the meta
  changes to Pause, Stop, Open deep dive (→ Session).
- Line 2: provider dot + `agent · model`, `· part`, subagent menu `N subagents ▾`
  (main + each subagent with its state). A pick shows that subagent's transcript.
- Body: transcript, anchored at the bottom, no scrollbar, 40 px fade at the top, idle
  panes at 60 % opacity. Line kinds: `x` meta, `u` user prompt (grey band, `› `),
  `a` agent text, `t` tool (`● Name` bold, green first letter), `o` tool output (`  └ `),
  `p` added line (green), `m` removed line (red), `s` subagent (purple `↳ name …`),
  `e` error (red).
- Ask box for `q` and `e`: label (Permission required, Plan approval, Question, Stopped on
  an error), text (mono for a permission), plan steps, option buttons (first = primary).
  An answer sets the session to working and appends the answer and the reply.
- Composer folds up on hover or focus (220 ms). With an ask it stays open as one compact
  line. Activity line (purple mono: `Running Bash · cmd`, `Waiting for your answer · N min`,
  `Idle · waits for a task`) and `ctx N % · $cost`. Controls: capture a screen area,
  attach, text, dropdowns mode (Plan, Edit, Auto), provider (a change = handoff), model,
  effort (low, medium, high, max), voice, send.

Quiet section: parked and done sessions as chips with Resume.

## 2. Session deep dive — `Session.dc.html`

- Header: back, breadcrumb `codebase / part · agent model`, title, state pill, buttons
  Pause, Park, Fork turn, Handoff, Stop.
- Stats strip: agents `1 + N sub`, turns, tool calls, cost, context %.
- Tabs: Waterfall, DAG, Transcript, Diff · N files.
  - Waterfall: span tree (run, turn, llm, tool, agent) on a minute axis, a blue "now" line,
    hatched running bars, search box, kind chips (dim others to 35 %), collapse per row.
    Kind colors: run #1A1A2E, turn #4B5BDB, llm #8B5CF6, tool #E8A23B, agent #0EA5A4.
  - DAG: 200×96 node cards, Bézier edges, dashed to waiting or planned nodes.
  - Transcript: same line kinds as the pane, with `TURN n · x min` dividers.
  - Diff: file list and a two-number-column diff.
- Quick pills: Continue, Run tests, Explain the last turn, Ask another session.
- Composer as in the pane, plus "to: main".
- Right 360 px "Span evidence": kind, name, status, 12 fields (agent, model, provider,
  start, end, tokens in/out, cost, tool, …), Input, Output, "Open in transcript",
  "Fork from here".
- Data: span `{id, parent, kind, name, agent, start, end, state, tokensIn, tokensOut, cost,
  tool, input, output}`; DAG node `{id, title, agent, duration, state, x, y, span}` and edges;
  diff file `{name, added, removed, lines[type, oldNo, newNo, text]}`.

## 3. Inbox — `Inbox.dc.html`

- Header: `N open · oldest waits X min`, filter All, Answers, Errors, Reviews, Conflicts
  (with counts), "Snooze all for 30 min".
- Left 420 px list of item cards, then "Done in this session" (60 % opacity).
- Right detail: `n of m` with Previous and Next; kind pill, title, wait, "Open deep dive →";
  meta line; transcript tail (full scroll, bottom anchored); conflict hunks side by side;
  ask card (text, plan steps, gate chips, options); composer "Or type your own answer…".
- An answer moves the item to Done with a result (allowed, denied, approved, merged, …)
  and selects the next item.
- Kinds: permission, plan, question (orange), error (red), review (green), conflict (purple).
- Data: item `{id, kind, title, where, agent, waitMin, cost, preview, text, options, steps,
  gates[label, ok], sides[{title, lines}], prompt, files, tail}`.

## 4. Tasks — `Tasks.dc.html`

- Header: `N tasks · N active · N queued for autopilot`, codebase filter chips, New task.
- Board columns Planned, Working, Waiting, Review, Done (Done folds to a 56 px strip).
- Card: title, `codebase / part`, wait reason (orange), 5-segment playbook bar, provider
  circles `N agents`.
- Right 360 px panel: playbook stepper, prompt, sessions (→ Session), gates, actions
  (Start now, Answer in the inbox, Review the diff, Open the merge, Open the session),
  Add a session, Hand off.
- Playbooks (5 steps): feature = Plan, Implement, Test, Review, Merge; fix = Reproduce, Fix,
  Test, Review, Merge; port = Inventory, Port, Fixtures, Review, Merge.
- Data: task `{id, column, title, codebase, part, playbook, step, waitReason, agents[],
  prompt, started, cost, sessions[], gates[]}`; per step: actor and duration.

## 5. Repos — `Repos.dc.html`

- Header: `N codebases · N daemons · N parts`, Add a repo, New project.
- Left 340 px repo list: name, state dots of its sessions, `N sessions · M parts`, daemon.
- Detail: path · branch · daemon · language, "Show in war room"; tabs Parts, Files, Activity.
  - Parts: Sankey (agents → parts → state, 1000×120), parts table (part, paths, live
    sessions, claims, last change, lines), "Re-map parts with an agent", "Add a part",
    "View map.toml" (syntax-colored, with author and age).
  - Files: tree with part tags and `claimed by <session>` pills.
  - Activity: time, provider · session, what, +added −removed, cost.
- `.aivory/map.toml`: `version`, `updated`, `[[part]]` blocks with `name` and `paths`.

## 6. Settings — `Settings.dc.html` (changes save at once)

- Nav: Providers and models, Autopilot, Display, Daemons and devices, Notifications,
  Account and license.
- Providers: card per provider (monogram, auth line, badge connected/ACP/off, enable
  switch, usage bars: orange above 75 %), default model, effort, permission mode,
  "Add an ACP agent", "Import from Claude Code settings".
- Autopilot: as in the war room popover, plus queue order (Priority, Oldest first,
  Smallest first).
- Display: theme (Light, Dark, System), panes per row, console font, font size, console
  background with preview.
- Daemons and devices: daemons (Restart, Open shell, Scale), phones (Remove), "Pair a phone"
  (QR, new code, pair by link).
- Notifications: events (Needs an answer, Error, Ready for review, Conflict, Autopilot
  action) × channels (Desktop, Phone, E-mail); quiet hours 22:00–07:00 (production errors
  still come through).
- Account: team, seats, license and renewal date, hub and relay status.
- Keys: `providers.<p>.{enabled, model, effort, mode}`, `autopilot.{on, parkAfterMin,
  maxRunning, maxPerCodebase, startQueued, order}`, `display.{theme, perRow, font,
  fontSize, background}`, `notify.<event>.<channel>`, `quietHours`.

## 7. Phone (Flutter, 390×844) — `mobile/`

Shared: 3 px rainbow strip, bottom tab bar Focus (badge), Sessions, Tasks, Code, Me.

- `Focus` — the cards that need you (permission, plan, question, error, review), each with
  a 3-line terminal tail, the ask, and answer buttons; swipe right to answer, left to
  snooze; done cards collapse to a line with Undo; voice button → Voice; daemon count chip.
- `Sessions` — pulse chart (40 min, mirrored bars), filter All, Needs you, Working, Idle;
  groups Needs you, Error, Working, Idle; row: title, `repo / part`, provider, `↳N`,
  time label, last line → Stream.
- `Stream` — one session: meta line, subagent switcher, Park, Fork, Handoff, Stop, status
  strip, bottom-anchored stream (user bubble, agent text, subagent line, tool row that
  expands to an inline diff), quick replies, composer (mic, mode, model, effort).
- `Approval` — permission detail: command, the agent's reason, risk note (biometric
  confirmation for risky kinds), history of similar answers, context grid, note to the
  agent; Approve once, Approve for this task, Deny with note; Face ID / fingerprint sheet.
- `Diff` — review: gates (tests, Guard, build, review), file pager, tap a line for
  "blame by prompt" (turn, prompt, model, session, time, cost), comment to the session as
  a ping; Request changes, Approve and merge.
- `Voice` — dictation to a task proposal: repo · part, playbook, agent, gates; When: Start
  now, Queue it, Plan only; waveform and hold-to-talk mic.
- `Me` — daemons, usage windows, monthly budget and warning, quiet hours, Live Activity,
  this phone (key in the OS keystore), offline answer queue, "Pair a desktop · scan the
  QR code".

## Mock values that need real data

Repos part claims, last change and lines; Tasks step actor and duration; the hardcoded
model, provider and codebase lists; the Inbox badge count (it differs between artboards);
"similar requests were approved N times" on Approval; usage-window percentages.
Most action buttons are unwired in the mocks; the implementation wires all of them.

---

# 8. Code intelligence screens — Proposed 2026-10-22, not yet accepted by the owner

New artboards for the lens, the file heat, the blame by prompt, the file timeline, and the
conflict radar (designs 03, 04, 05). They use the tokens of the accepted design. Where a
choice is new, this section names it. The owner must accept this section before the UI work
treats it as final. Both files use the `.dc.html` format. Each file shows several frames
(desktop: in a column, phone: in a row), so that every state has a picture. Each frame
has the id `frame-<name>`.

## Inventory

| File | Frame | Size | State shown |
|---|---|---|---|
| `Lens.dc.html` | 1 Lens | 1440×900 | Outline tab; the hover card "blame by prompt" on line 23 |
| | 2 Lens | 1440×900 | Problems tab; the hover doc card on the symbol `total` (line 28) |
| | 3 Lens | 1440×900 | Timeline tab; a click on turn `8a22 · 5` shows its diff in the code view |
| | 4 Lens | 1440×900 | Radar tab; the usages list under the header (after Find usages) |
| | 5 Repos · Radar | 1440×900 | Fourth tab of the Repos detail: graph and hit list |
| `mobile/Code.dc.html` | 1 Files | 390×844 | Code tab, Files segment: picker, file list |
| | 2 File | 390×844 | Read-only code, blame card after a tap on line 23 (pushed screen, no tab bar) |
| | 3 Radar | 390×844 | Code tab, Radar segment: pairs at risk |

`Lens.dc.html` is one tall board (1440 × 4720, five frames with a 36 px caption each).
`mobile/Code.dc.html` is one wide board (1250 × 880, three frames). `canvas.json` is not
changed yet; add the two boards when the owner accepts the section.

Entry points: Repos → Files (a click on a file), and Session deep dive → Diff or Transcript
(a click on a path). Both open the Lens. The header shows `from session <id>` when the Lens
opens from a session. The phone tab "Code" opens `mobile/Code.dc.html`. In the accepted
files it opens `Diff.dc.html`; Diff stays the review screen of a task.

Sample data is synthetic: codebase `billing`, file `billing/invoices/invoice.rs`, symbol
`billing::Invoice::total`, sessions `7f3a` (claude, payment, you), `8a22` (gemini, invoices,
you), `8d07` (claude, database, you), `m2e5` (codex, invoices, Mara), `k9d4` (opencode,
payment, Jonas). The sample code, turns, and claims are in the script block of each file.

## New tokens

Syntax colors (token kinds). The tokenizer in the script block is a mock. The app uses
tree-sitter captures plus LSP semantic tokens.

| Kind | Color | Capture (examples) |
|---|---|---|
| keyword | #4B5BDB | keyword, storage |
| type | #0D9488 | type, type.builtin, constructor |
| function | #6D28D9 | function, function.method |
| string | #15803D | string |
| number | #C2410C | number, constant.numeric |
| comment | #8A93A6 | comment, doc comment |
| attribute, macro | #5A6578 | attribute, macro |
| other | #1A1A2E | variable, property, punctuation |

Turn blame column: 4 px wide, one color per line, chosen by the age rank of the turn inside
this file (rank 0 is the newest turn): #3574F0, #6B8AFF, #A9BCFA, #CBD5F5, #E3E7EF (rank 4
and older, also commits). The legend in the code header shows the five colors, "new → old".

Diagnostics: error #DC2626 (filled dot, red wavy underline), warning #F59E0B (filled dot,
amber wavy underline), hint #8A96A8 (hollow ring, dotted underline). A "new in turn N" tag
uses bg #EEF3FE with text #2C4FA8.

File heat: a 3 px bar at the right edge of the gutter, #4B5BDB. An `editing` claim has full
opacity, an `intent` claim 35 %. The lines get a tint (5 % and 2.5 %). The first line of the
range shows a claim pill at the right (`8a22 editing`, `8d07 intent`). Only the claims of
other sessions than the opening session count. A `landed` claim shows no heat.

Radar risk (the score of design 04): 0.1 #A0AEC0 (text #5A6578, bg #F1F3F6), 0.4 #F59E0B
(text #92400E, bg #FFF8EB), 0.7 #F97316 (text #C2410C, bg #FFF1E6), 1.0 #DC2626
(text #B91C1C, bg #FDECEC).

## Lens — `Lens.dc.html` frames 1 to 4

Header, 60 px: back to Repos, `billing / invoices · main`, file name (16 px, 600), pills
`Rust`, `claimed by <session>`, `from session <id>`; buttons "Go to symbol ⌘O" and "Open
session <id>". The body is a grid: 300 px, 1fr, 320 px. The rail marks Repos.

Left, file tree (300 px, white): the filter "Go to file ⌘P", the line `billing · main · N
parts`, rows of 30 px as in Repos → Files (part tag on a folder, `claimed by <session>` pill
on a file, an orange pill when that session needs you). A red dot with a count at the right
shows the problems of a file. The open file has the #EEF3FE row.

Center, code view. Header 40 px: the symbol path at the cursor (`Invoice › total`), `Rust · N
lines · rust-analyzer ready`, the blame legend. Lines are 22 px high, JetBrains Mono 12 px.
Columns: blame 4 px, line number 38 px (right aligned, #A0A7B5), gutter 24 px (diagnostic mark
8 px at x 3, heat bar at the right edge), code. A line under the pointer has #F8F9FB.
Diagnostic ranges get the underline of their severity.

Diff mode (frame 3): the header shows `session · turn n`, model, time, cost, `+a −r`. A 52 px
bar shows the prompt in quotes and the buttons "Revert turn", "Fork from this turn", "Back to
file". The lines have two number columns (40 px each) and a marker column (18 px). The colors
are the colors of Session → Diff (added #F0FAF3 with #15803D, removed #FEF3F3 with #B91C1C,
hunk #F3F5F8).

Cards: white, radius 12, 1 px line, shadow `0 12px 32px rgba(13,13,14,.14)`, padding 14×16,
gap 8. A card opens under its line (above the line from line 26 on, so that it stays in the
view). It closes when the pointer leaves the line and the card.
- Blame card, 380 px: kicker `BLAME BY PROMPT · TURN <session> · <n>` (mono 9.5 px, caps,
  #2C4FA8); the prompt in quotes; a mono line `model · session <id> · HH:MM · $cost`; buttons
  "Revert turn" (red text) and "Fork from this turn". A line from a commit shows `BLAME ·
  COMMIT <sha>`, the message, author, time, and one button "Show in timeline".
- Doc card, 420 px: kicker with the symbol; the signature in token colors; the doc text
  (12.5 px, #4A5568); a mono line `file:line · N usages · N sessions`; buttons "Go to
  definition" (primary) and "Find usages". The symbol under the pointer gets #EEF3FE.
- Usages list (frame 4): it drops from the header edge over the code column (left 300 px,
  right 320 px, radius 0 0 12 12). Header 44 px (`Usages of Invoice::total`, `N in N files`,
  close). A group row of 30 px (#F6F7F9) per file: path, part tag, claim pill. A result row of
  28 px: line number, the line with the symbol on #EEF3FE, a red dot when the usage has a
  problem. Footer 34 px with the keys. Enter opens the result in the code view. Esc closes.

Right panel (320 px, white). Tabs Outline, Problems (red count), Timeline, Radar (purple
count), as the Repos tabs at full width.
- Outline: "Structure" (rows of 30 px: kind glyph, name, line range, claim pill, red dot when
  the symbol has a problem), "Callers of total" (call hierarchy), "Tests of total" (state
  dot). A click scrolls the code view to the symbol.
- Problems: "This file" (dot, message, source and line, tag `new in turn N`), "Other files"
  (problems that a turn caused elsewhere, with the claim pill of the owner), "Diagnostics run"
  (the five stages of design 03 with the time; a stage that is off shows a hollow ring).
- Timeline: points, newest first. A turn point has a dot in its blame color. A gate or
  review point has a green dot. A commit has a hollow ring. Row: `who`, time, `+a −r`, text,
  mono detail. A click shows the diff of that point in the code view. A drag over two points
  shows the range diff. "Back to file" returns.
- Radar: "Claims on this file" (session, state, lines, lease), "Radar hits" for the pairs that
  involve this file (score pill, pair, tier, key, "Open both", "Send ping"), "Pings" (last hour).

## Repos → Radar — `Lens.dc.html` frame 5

The fourth tab `Radar` has an orange count (the pairs at 0.7 and up). It sits next to Parts,
Files, Activity. Content under the tabs:
- Graph card. Title `sessions → shared keys`, meta `N sessions · N pairs · N at risk`, the
  legend (four risk swatches). The box is 928×240. A node is 160×50, radius 12, card
  shadow: provider dot, session id (mono 12, 600), state dot at the right, `part · owner`
  (mono 10.5). An edge is a curve in the risk color, 2 to 4 px wide (wider for a higher
  score), with the score pill at the middle (white, 1.5 px border in the risk color). The
  pill shows the routing key on hover.
- Hits card. Header `Hits · N pairs · sorted by score · trial merge with git merge-tree`. A row
  of 62 px: score pill; `a ↔ b` with provider dots and the tier text (`Tier 1 · same path`,
  `Tier 2 · same symbol`, `Tier 3 · dependency`, then the result of the trial merge); two
  routing keys (mono 11); buttons "Open both", "Send ping", "Pause one ▾". A row at 0.1 has
  60 % opacity.
- A click on a node selects its edges and dims the rest. A scope switch (This daemon, Team) is
  not drawn. The sample shows the team scope.

## Phone — `mobile/Code.dc.html`

- Files and Radar frames: title "Code" (30 px, 700), a sub line, a search button. A picker row
  of two buttons (44 px, radius 12): `REPO billing ▾`, `PART invoices ▾`. A segmented control
  Files | Radar (orange count).
  - Files: a group header `PART · N files · N problems`, then a card (radius 16) of rows (64
    px): file name (mono 14, 600), `dir · N lines`, claim pill, red dot with the problem
    count, chevron.
  - Radar: the header `AT RISK · N pairs · score 0.4 and up`, then one card per pair
    (radius 16): score pill, `a ↔ b` with provider dots, `with <owner>`, tier text, routing
    key (mono 11.5), buttons "Send ping" (primary), "Open both", "Pause one". A last row folds
    the pairs under 0.4.
- File frame: back, file name, `repo / part · Rust · N lines`; chips (problem count in red,
  the heat ranges with the heat bar). The code has the blame column (4 px), the line number
  (34 px), the gutter (16 px), and the code (it scrolls sideways, no scrollbar). A tap on a
  line opens the blame card of Diff (#EEF3FE, radius 12) with "Comment to the session" (a
  ping), "Fork from this turn", and close. "Revert turn" is not on the phone. Footer: "Next
  problem · n of N" and "Ask session <id>".

## Data fields and RPC methods

On 2026-10-22 `aivory-protocol` has `code/file`, `code/symbols`, `code/definition`,
`code/references`, `code/hover`, `code/diagnostics`, `ledger/blame`, `ledger/turns`,
`ledger/timeline`, `ledger/revert`, `session/fork`, `session/pause`, `repos/get`,
`repos/map`. Check the fields against the table and change the protocol first where they
differ. The methods marked "later" do not exist.

| Element | Fields | Method |
|---|---|---|
| File tree row | `path`, `kind`, `part`, `claims[{session, state}]`, `problems` | `repos/get` + `claims/list` (later) + counts of `code/diagnostics` |
| Code lines | `path`, `rev` (working tree or snapshot), `lines[{no, text}]`, `tokens[{line, from, to, kind}]`, `lang` | `code/file` |
| Gutter marks | `diagnostics[{line, col, endCol, severity, message, source, rule, fingerprint, addedInTurn}]` | `code/diagnostics` |
| Outline | `symbols[{name, kind, range, children}]`; callers `[{symbol, path, line}]`; tests `[{key, state}]` | `code/symbols`, `code/references` |
| Doc card | `signature`, `doc`, `defined_at {path, line}`, `usages`, `sessions` | `code/hover` |
| Go to definition, Find usages | `{path, line, col}` in; `locations[{path, line, text, part, claim, problem}]` out | `code/definition`, `code/references` |
| Blame column and card | per line `{turn: {session, n, prompt, model, started_at, cost_usd}, commit}` | `ledger/blame` |
| Timeline | `points[{kind: turn, commit, gate, or review; id, time, who, text, added, removed, cost_usd, snapshot_ref}]` | `ledger/timeline` |
| Diff of a point | hunks as in the Session diff; `range {from, to}` for a drag | `ledger/timeline` with a diff option |
| Revert turn | `{session, turn}`; the ledger records an operation | `ledger/revert` |
| Fork from this turn | `{session, turn, provider?}` | `session/fork` with `at` |
| File heat, claim pills | `claims[{session, owner, state: intent, editing, or landed; key, range, lease_s}]` | `claims/list` (later) |
| Radar graph and hits | `sessions[{id, provider, part, owner, state}]`, `pairs[{a, b, score, tier, keys[], merge: clean or text, shared_symbols}]` | `radar/list` (later) |
| Open both | two session ids; opens two panes in the war room | client only |
| Send ping | `{to[], kind: fyi or question, key, text}` | `mail/send` (later) |
| Pause one | `{session}` | `session/pause` |
| Ask session (phone) | `{session, text}` | `mail/send` (kind question, later) |
| Comment to the session (phone) | `{session, path, line, text}` | `mail/send` (kind fyi, with the quoted line, later) |

## Interactions

- A hover on a line shows the blame card. A hover on a symbol shows the doc card (the blame
  card hides). A hover on a heat bar shows `<session> · <state> · lease N min`.
- Cmd-click on a symbol: go to definition. "Find usages" opens the list under the header.
  Arrow keys move, Enter opens a result, Esc closes.
- A click on a Problems or Outline row scrolls the code view to the line. A click in the tree
  opens that file. The address of the Lens is `repo + path + (turn or commit)`.
- A click on a Timeline point opens diff mode (frame 3). "Fork from this turn" starts a new
  session at that snapshot (`session/fork` with `at`). "Revert turn" asks for a confirmation,
  restores the snapshot before the turn in the session workspace, and shows the result in the
  Timeline.
- Every lens feature also works in diff mode and on a historical turn (design 03).
- The panel tab, the open card, and the diff point are view state. They change nothing in the
  repository.


## Session pane body v2 (owner feedback 2026-10-22)

The owner looked at the running app: the panes "should and must look more like terminals",
"clear and focused", "they don't even have padding", "not sure if you support markdown there
yet, or any of the other proper features". This section replaces the pane body of
`Main.dc.html`. The header, the grid, the rules (light theme first, square pane corners, color
only for state, details on hover, no visible scrollbars, calm) stay. The model for the look is
the output of Claude Code, Codex and Gemini: tool call rows, indented results, diffs,
prompts, permission dialogs, a status line. The code is `apps/desktop/src/console/`; the
war room panes, the deep dive Transcript and the Inbox detail tail all draw from it.

### Surface

| Value | Setting |
|---|---|
| Font | the Display font for everything in the body, the asks and the prompt box included |
| Size | the Display size (default 12 px); mini panes -2 px, micro panes -3 px |
| Line height | 1.5 x size; one character is 0.6 x size wide (all five console fonts) |
| Padding | 14 px left and right, 12 px top and bottom (mini 10 / 8, micro 8 / 6) |
| Background | Display "background": Light #FBFAF7 (warm paper, the default; was White #FFFFFF), Grey #F4F5F7, Dark #15161B, Navy #0D1624 |
| Ink | text #1A1A2E, dim #5A6578 (dark: #E4E5EA, #9096A5) |
| Tints | prompt and code #F1EFE9 (grey theme #E8EBF0 and #E9ECF0; dark white 7 %); bar #C9C5BB; link #2F5BD3 (dark #8AB4FF) |
| Top and bottom edge | a fade of padding + 0.7 lines at an edge that cuts the list (solid for the first 45 %), so no line shows half cut; the fade is off at the very top and bottom; with older lines the first row is the marker `↑ N earlier lines` (a click loads them) |

The hierarchy comes from indent and weight: agent text at the left edge, a tool call at the
left edge with a bold name, results 2 characters in and dim, sub-results 4 characters in.

### Blocks

- Agent text: markdown (pulldown-cmark, MIT). Headings (bold, h1 1.2 x, h2 1.1 x), bold,
  italic, strike, inline code (tint), fenced code (tinted block, 4 px radius, tree-sitter colors
  from `aivory-syntax`), bullet and numbered lists (nested), task lists, block quotes (2 px bar,
  dim), links (click opens the browser), tables (columns by widest cell), rules. Inline code
  that names a file is underlined and opens the Lens. A line break in the text stays a line
  break. The parse of a text is cached.
- Tool call: `● Name(argument)`. The dot is 0.6 x size: green when done, red on an error,
  orange when the call waits for an answer, and it pulses (4 steps per second) while it runs.
  A file path in the argument is underlined and opens the Lens. Results sit under a drawn `⎿`
  bend, 2 characters in. A result folds to 4 rows; the marker `… +N lines` is a click target
  (`… fold` folds again). A diff folds at 12 rows.
- Diff: rows tinted green or red (add / del colors of the theme), a `+`/`-` gutter, and a
  line number column when the numbers are known (a `@@ -a,b +c,d @@` header, or a `Write` call
  from line 1). The protocol has no numbers for an `Edit`: the column stays out.
- Output colors: ANSI SGR (colors 30-37, 90-97, 38;5, 38;2, bold, dim, italic, underline) are
  parsed; backgrounds and all other escape codes are stripped.
- Prompt: `> text` on a tint with a 2 px bar at the left, 12 px above.
- Subagent line: `↳ name  text` in purple, as before. The switcher in the header is unchanged.
- Todo list: a `TodoWrite` call with a list (JSON in the argument, or `[x] [~] [ ]` rows in the
  result) shows as a checkbox list: done is filled and struck through, active is bold with a dot.
- Status line, under the transcript of a working pane: `✻ Working… 12s` (spinner glyph with 4
  steps per second, verb from the last tool, elapsed time, `· 1.2k tokens` when known).

### Asks and the prompt box

- Ask: a bordered box (1 px, 4 px radius) in the console font. Title in bold (Permission
  required, Plan, Question, Error), the command in a tint, plan steps as bullets, and numbered
  options. A permission reads `1. Yes`, `2. Yes, and don't ask again for <command>`,
  `3. No, tell the agent what to do`. The keys 1-9 answer while the pane has the focus and the
  prompt box is empty.
- Prompt box: a bordered `> ` box in the console font (4 px radius). A pane with room
  (up to 3 per row) shows it all the time; hover or focus unfolds the controls.

### Focus and keys

- A click on a pane focuses its prompt box: a 2 px accent outline over the border (no layout
  shift). Typing goes to the prompt box. PageUp and PageDown scroll; Home and End scroll while
  the prompt box is empty; Esc leaves the pane; Ctrl+C with an empty prompt box copies the
  transcript text.
- Mouse selection of text inside a body is open: GPUI has no selectable text, and a selection
  across the blocks needs its own hit testing and a text model (more than a day).

### Dev flags

- `--offscreen`: opens the main window without focus at (-20000, -20000), outside all
  monitors, and never activates it. Use it with `--demo`.
- `--scroll-row N`: scrolls the war room to row N at start (1 is the first group of panes).
- `--demo-rich` (with `--demo`): the first two panes that wait for an answer and the first
  working pane show made-up transcripts with markdown, a diff, colors, a folded result and a todo list.
