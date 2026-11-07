# ADR 0024 — Theme packages: looks as data, a registry, and install from GitHub

Status: Accepted, 2026-11-07. Changes parts of ADR 0022 (see "Changes to other ADRs"). The
cuts of version 1 are in "Cuts in version 1".

## Context
ADR 0022 makes a look an app-wide theme with one read model and one action API. The looks are
Rust code in the desktop app. ADR 0023 adds more displays: GPUI desktop, iPad, phone, foldable
iPhone and Apple Watch, first in Flutter, then in SwiftUI and Jetpack Compose.

The owner wants (2026-11-03):
- Users create their own themes, publish them on GitHub, and install them from a GitHub link.
- A marketplace like shadcn/ui: registries of themes and of smaller blocks. A user adds an
  item, and then owns and edits the copy.
- A theme works without compilation, on every display.

With looks as code, each look costs one build per display (four technologies), and a user
cannot add a look. The accepted looks of `docs/design/looks/` (Minimal, Bridge, Operator,
Neon, Navigator, Holo, Tactical) must stay possible.

## Decision
A theme is a data package. Each display draws it with a fixed catalog of compiled components.
The layout of every screen is data. Themes are data only: no scripts, no network, no files.

### 1. Package layout
A theme is one folder:

```
my-theme/
  theme.toml                 manifest
  tokens.toml                colors, fonts, text sizes, space, radii, motion, effects
  layouts/
    desktop/                 desktop and iPad (required)
      warroom.json  session.json  inbox.json  tasks.json
      repos.json    settings.json focus.json  decision.json
    phone/                   phone (optional)
    fold/                    foldable phone, unfolded (optional)
    watch/                   Apple Watch: warroom.json, decision.json only (optional)
  parts/                     named layout parts that layouts reuse (optional)
  assets/
    fonts/                   .ttf, .otf, with a LICENSE file in the folder
    images/                  .png, .jpg, .webp, static .svg, with a LICENSE file
  previews/                  desktop.png (1440x900), phone.png (optional)
  README.md  LICENSE
```

The manifest `theme.toml`:

| Key | Content |
|---|---|
| `id` | lowercase, `[a-z0-9-]`, 3 to 40 characters |
| `name`, `description` | text for Settings and the gallery |
| `version` | SemVer, equal to the Git tag without the `v` |
| `author` | name, and an optional URL |
| `license` | an SPDX expression for the theme files |
| `min_app` | the lowest app version (SemVer) |
| `catalog` | the component catalog version that the theme needs (integer) |
| `displays` | the display classes that the theme supports, `desktop` is required |
| `tokens` | the token files in order; a later file overrides keys of an earlier file |

Display classes:

| Class | Devices | When a theme has no layouts for the class |
|---|---|---|
| `desktop` | GPUI desktop app, iPad | required |
| `phone` | phone, folded foldable | the Minimal phone layouts with the theme tokens |
| `fold` | foldable phone, unfolded | two panes from the `phone` layouts: list and session |
| `watch` | Apple Watch | the Minimal watch layouts with the theme colors |

Screens: `warroom`, `session` (session detail), `inbox`, `tasks`, `repos`, `settings`,
`focus` (Focus view), `decision` (the decision dialog). The `watch` class has `warroom` (the
waits list) and `decision` only.

### 2. Tokens
`tokens.toml` holds named values. Components read them by name; a layout refers to a token
name, never to a raw value.

- `colors`: `bg`, `surface`, `text`, `muted`, `accent`, `analysis`, and `state.*` with one
  color for each session state of the protocol: `needsYou`, `working`, `idle`, `parked`,
  `error`, `done`. Components color a session by its state. Thus color stays for state only.
- `fonts`: `ui` and `mono` families. A family is a bundled file or a generic name (`sans`,
  `serif`, `mono`).
- `type`: the five text sizes `xs`, `sm`, `md`, `lg`, `xl` (the type system of the app).
- `space`: named gaps and margins. `radii`: named corner radii. The built-in Minimal theme
  keeps the radii 4, 8, 12 and 18 px and square session panes.
- `motion`: durations in ms and an easing name. All displays obey the "reduce motion"
  setting of the OS.
- `effects`: glow, scanline strength, glass fill alpha, blur radius, each with a fixed range.

### 3. Layout format: JSON
A layout file is a JSON component tree. We use JSON for these reasons. The tree is deep, and
TOML is weak for deep trees. JSON maps one to one to the JSON Schema, so the schema checks a
layout with no conversion. An author adds `"$schema"` and gets completion and errors in VS Code
and JetBrains IDEs with no plugin. Rust, Dart, Swift and Kotlin read JSON with their standard
tools. The manifest and the tokens stay TOML, because they are flat and people edit them by hand.

A node:

```json
{ "type": "panel", "id": "fleet", "variant": "glass", "title": "Fleet",
  "when": { "path": "$asks", "is": "nonEmpty" },
  "children": [ ... ] }
```

- `type` names a component of the catalog (section 4). Each component has its own fixed props.
- A string that starts with `$` is a data path into the snapshot, for example
  `$daemon.version`. Inside `repeat`, `list` and `sessions`, `$item` is the current element. `$$`
  writes a literal `$`. There are no templates, no operators, no functions and no general
  expressions.
- `when` shows a node only if one test is true. Tests: `nonEmpty`, `empty`, `eq`, `ne`, `in`.
  A test has one path and one literal value.
- `map` picks a token or a variant from a table, for example
  `{ "path": "$item.state", "map": { "needsYou": "alert" }, "else": "plain" }`.
- `{ "use": "<part>" }` inserts a part from `parts/`. Parts do not refer to themselves.
- The model computes all counts, orders and text formats (ADR 0022). A layout shows them; it
  cannot sort, filter or compute.

Data roots (the snapshot of ADR 0022 section 2, in the protocol per ADR 0023):
`codebases`, `sessions`, `asks`, `gates`, `usage`, `events`, `inbox`, `tasks`, `repos`,
`daemon`, `analysis`, `intent`, plus the screen context `selected` (the selected session) and
`display` (`class`, `width`, `tier`).

### 4. The component catalog
Every display implements every component of the catalog for its classes. The catalog has a
version number; a new component increments it.

| Group | Components |
|---|---|
| Layout | `stack` (row or column), `grid`, `layer` (overlay), `scroll`, `spacer`, `repeat`, `responsive`, `sessions` |
| Frame | `frame` (Bridge frame with segments), `panel` (variants: flat, glass, chamfer, window, monitor), `nav`, `segment_button`, `button`, `switch`, `tabs`, `look_switch`, `key_hint` |
| Content | `text`, `readout` (label and value), `badge`, `image`, `list`, `row`, `card`, `log`, `files` (+/- bars), `diff` |
| Sessions | `decision` (decision card), `decision_queue`, `transcript` (chat), `terminal`, `composer`, `settings_section` |
| Charts | `chart` (kinds: line, area, bar, strip, stacked, spark, segments), `gauge` (kinds: arc, bar, dial), `ruler` |
| Effects | `rain`, `scramble`, `scanlines`, `rings` |
| Scenes | `city3d`, `fsn3d`, `radar`, `bloom`, `camera_wall` |

Rules:
- A scene takes parameters from fixed lists and ranges. Examples: `city3d` `height` is one of
  `tokens`, `cost`, `turns`; `radar` `sweep_s` is 2 to 20 and `rings_min` is a list of up to
  6 values; `bloom` `strand` is `tokensHour` or `tokensTotal`; `rain` `speed` is `activity`
  or a fixed rate.
- Each scene and each effect node has a `fallback` child. A display that cannot draw the
  scene (for example the Watch) draws the fallback.
- App-owned content: `nav` lists the screens of the app; `settings_section` draws the
  sections of Settings; `decision` draws the question, the reason, the risk flag and the
  options of the ask from the snapshot. A theme picks a variant and a place; it cannot change
  this content. Settings > Display is always present.
- Analysis items (`$analysis.*`) are valid only inside a `panel` with `"role": "analysis"`.
  That panel shows the label "Analysis" (ADR 0022 section 2).
- Availability per class: `watch` has `stack`, `list`, `row`, `text`, `readout`, `badge`,
  `gauge`, `decision`, `composer` (dictation) only. All other classes have the full catalog.

### 5. Controls and actions
A control maps to one action of the action API (ADR 0022 section 3), and to nothing else:

```json
{ "type": "button", "label": "Open session",
  "on": { "press": { "action": "focus", "id": "$item.id" } } }
```

- Actions: `answer`, `focus`, `dispatch`, `newSession`, `send`, `pause`, `park`, `stop`,
  `resume`, `autopilot`, `permissionMode`, `open`, `setLook`.
- An argument is a data path or a literal from the fixed values of that argument.
- Text for an agent (`send`, `dispatch`, `newSession`, `answer` with text) comes only from a
  `composer` that the user types into. A theme cannot put a fixed prompt on a button.
- The options of an ask come only from the `decision` component. A theme cannot answer an ask
  by itself and cannot relabel an option.
- An action fires only on a user event: `press` or a key. There are no timers and no load
  events. Keys cannot use the reserved keys of the app (`Ctrl+Shift+L`, `Esc`).

### 6. Layout rules for 1 to 30+ sessions
- The `sessions` component holds one template per density tier of ADR 0022 section 6:
  `"1"`, `"2-4"`, `"5-12"`, `"13-30"`, `"31+"`. A tier without a template uses the next
  smaller tier.
- The tiers `13-30` and `31+` must use the list layout (`layout: "list"`, grouped by codebase).
  Thus a scene shows at most 12 sessions; from 13 sessions the theme shows rows.
- The model gives the order: sessions with an ask stay first. In `31+`, the app closes the
  groups without an ask or an error.
- `responsive` holds one node per width class: `compact`, `regular`, `wide`. A class without
  a node uses the next smaller class.
- Text clips with an ellipsis and shows the full text on hover. Components do this; a theme
  cannot turn it off.
- `phone` layouts get the classes `compact` only. `fold` layouts get `compact` and `regular`.

### 7. Validation
- Rust types in a new module `crates/aivory-protocol/src/theme.rs` define the manifest, the
  tokens, the layouts and the registry. `schemars` generates
  `proto/schema/aivory-theme.schema.json` and `proto/schema/aivory-registry.schema.json` in
  the same bless step as the protocol schema (`AIVORY_BLESS=1 cargo test -p aivory-protocol`).
  The site publishes both at a fixed URL for `$schema`.
- The validator (`aivoryd theme check <dir>`) runs the schema, then the checks that a schema
  cannot do: data paths exist in the snapshot types, actions and arguments are valid, the
  required screens exist, `parts` have no cycles, assets exist and have a license file, the
  tiers follow section 6, caps (section 8) hold, and the tag equals the version.
- An error names the file, the JSON pointer and the rule, for example
  `layouts/desktop/warroom.json /root/children/2/on/press/action: unknown action "delete"`.
- The app checks a theme on add, on update and on each load. It rejects an invalid theme. If
  the current theme fails on load, the app uses Minimal and shows the message.
- The built-in themes are test fixtures: `cargo test -p aivory-protocol` checks each of them.

### 8. Safety
- A theme is data only: TOML, JSON, Markdown, fonts and images. The validator rejects all
  other file types. SVG must be static: no `script`, no `foreignObject`, no event attributes,
  no external `href`.
- A theme has no network access, no file access and no access to the clipboard. It reads only
  the snapshot; it cannot add data paths or change the action API.
- Caps: archive 10 MB; unpacked 20 MB; 500 files; one asset 4 MB; one layout 512 KB;
  4,000 nodes and depth 40 per layout; 8 font families; images up to 4096x4096 px; one text
  literal 200 characters; a registry index 2 MB and 2,000 items.
- The archive check rejects links, absolute paths, `..` paths and a compression ratio over
  100:1.
- Hash pin: for each added version, `theme.lock` in the theme folder records the source, the
  version, the SHA-256 of the archive and the SHA-256 of each upstream file. The same version
  with another hash is an error (a moved tag).
- `aivoryd` is the only process that downloads. It uses HTTPS only, and only on a user
  command: add, update, diff, or a registry refresh. There are no background downloads.
- The look switch (`Ctrl+Shift+L`) and Settings > Display belong to the app. The user can
  always go back to Minimal. `aivory --look minimal` starts with Minimal.
- Real session data never leaves the machine through a theme. Previews use demo data.

### 9. Install, registries and blocks
The model is the model of shadcn/ui: an add copies the files, and the user owns the copy.

Sources:
- `@<registry>/<name>[@<version>]`: an item of a registry.
- `github:<owner>/<repo>[@<tag>]`: a GitHub repo. Without a tag, the latest release. The
  service takes the release asset `*.aivory-theme.zip`; if there is none, the zip of the tag.

Registries:
- A registry is a static file `registry.json` on GitHub, GitHub Pages or any HTTPS host. The
  schema is `proto/schema/aivory-registry.schema.json`.
- Each item has: `name`, `type`, `version`, `title`, `author`, `license`, `description`,
  `preview` (image URL), `source` (a GitHub release asset, or a list of raw file URLs),
  `sha256`, `min_app`, `catalog`, `displays`, and `requires` (other items).
- The official registry is `@aivory`. Other registries need a name and a URL:
  `aivoryd registry add @someone https://someone.github.io/aivory/registry.json`.
- Item types: `theme` (a full package), `screen` (one screen layout for one or more display
  classes), `parts` (a set of layout parts, for example a decision card), `tokens` (a token
  set).

Commands (all also run from Settings > Display):

| Command | Effect |
|---|---|
| `aivoryd theme init <id> [--from <theme>]` | makes a starter folder from Minimal or another theme |
| `aivoryd theme add <source>` | downloads, checks the hash and the theme, copies to the themes folder, writes `theme.lock` (`install` is an alias) |
| `aivoryd block add <source> --to <theme>` | copies a block into a theme; asks before it replaces a file; a token set goes to the end of `tokens` |
| `aivoryd theme list` | shows the themes, their source, version and local changes |
| `aivoryd theme diff <id>` | shows local edits and upstream changes |
| `aivoryd theme update <id>` | merges upstream changes with a three-way merge; a conflict keeps the local file and writes `<file>.upstream` |
| `aivoryd theme remove <id>` | removes the folder; the current theme falls back to Minimal |
| `aivoryd theme check <dir>` | runs the validator |
| `aivoryd theme dev <dir>` | checks and reloads the theme on each save, in all connected displays |
| `aivoryd theme pack <dir>` | makes `<id>-<version>.aivory-theme.zip` and prints its SHA-256 |
| `aivoryd registry build <dir>` | makes `registry.json` and the item archives from a theme repo with `registry.toml` |
| `aivoryd registry add / list / remove` | manages the registries of the user |

- Preview: `aivory --demo --demo-rich --display <class>` shows the layouts of one display class
  in a window of that size. The desktop app implements the full catalog, so it can show all
  classes. `theme dev` shows each validator error in a panel over the screen.
- The themes folder is `themes/` in the per-user data folder of `aivoryd`, next to
  `aivory.db`. Tests use `AIVORY_CACHE_DIR`.
- Settings > Display shows the installed themes as cards with a preview image, the source,
  the version and an update badge. A field accepts an `@registry/name` or a GitHub link, and
  a dialog shows name, author, license, size and hash before the add.
- The website has a gallery page that reads the official `registry.json`: browse, preview,
  and copy the add command.

### 10. How displays get a theme
- `aivoryd` checks the theme and serves it as one resolved bundle over JSON-RPC, on every
  transport (pipe, LAN link, iroh): `theme/list`, `theme/get` (manifest, merged tokens and
  the layouts of one display class with parts inserted), `theme/asset` (one asset by hash),
  `theme/add`, `theme/update`, `theme/remove`, and the note `theme/changed`.
- Displays read JSON only. They do not parse TOML and do not check themes. They keep assets
  in a cache by hash. One asset stays below the 8 MB frame limit of ADR 0021.

### 11. Built-in themes
- The eight looks ship as theme packages in a new MIT folder `themes/` (minimal, bridge,
  operator, neon, navigator, holo, tactical, command). The `aivoryd` binary contains them.
  The Command look joined after this ADR was drafted; it follows the same rules.
- They are the proof of the format: each must match its artboard of `docs/design/looks/`.
- They are the items of the `@aivory` registry. Their fonts (for example B612, OFL) keep a
  license file next to them.

### 12. What stays compiled
- The renderer of each catalog component, once per display: GPUI, Flutter, SwiftUI, Compose.
- The read model, the action API, the layout engine, the validator and the scenes.
- A theme that needs a new component, a new data path or a new scene parameter asks for it
  upstream (an issue in the AIVory repo). The component lands in all displays, then the
  catalog version increments.

### 13. Props of the catalog (desktop, catalog version 1)
Props that name a token (`gap`, `pad`, `radius`, `bg`, `border`, `color`, `size`) read the
tables of `tokens.toml`; an unknown name gives the default. A number is a size in px. Every
node takes `id`, `when`, `grow` (a number over 0 fills the free space), `width`, `height`,
`pad`, `bg`, `radius` and `border`.

| Component | Props |
|---|---|
| `stack` | `dir` (row, column), `gap`, `align`, `justify`, `wrap` |
| `grid` | `columns`, `gap` |
| `layer` | the first child is the base; the others lie over it |
| `scroll` | `dir`, `gap` |
| `spacer` | `size` (without it: fills) |
| `repeat` | `items` (a list path), `limit`, `dir`, `gap`, `wrap`; children are the template |
| `list` | `items`, `limit`, `gap`, `empty` (text); children are the template; `$item` is valid |
| `responsive` | `classes` |
| `sessions` | `tiers`, `gap`, `include` (`all` shows parked and done), `item_width` |
| `panel` | `variant` (flat, glass, chamfer, window, monitor), `title`, `role` |
| `frame` | `segments` (a list of labels), `title` |
| `nav` | `dir`, `variant` (pill, plain), `gap` |
| `button`, `segment_button` | `label`, `variant` (primary), `selected`, `composer` (the id of the composer whose text a `dispatch` or `newSession` sends), `on` |
| `switch` | `label`, `value`, `on` |
| `tabs` | `items` (labels); children are the pages |
| `look_switch`, `key_hint` | `keys`, `label` |
| `text` | `value`, `size`, `color`, `weight` (bold), `mono`, `align`, `lines` |
| `readout` | `label`, `value`, `unit`, `color`, `dir` |
| `badge` | `value`, `state`, `tone` |
| `row`, `card` | `title`, `sub`, `meta`, `state`, `on` |
| `log` | `items`, `field`, `limit` |
| `files` | `label`, `added`, `removed` |
| `diff` | `value` |
| `chart` | `kind`, `items`, `field`, `max`, `height`, `color` |
| `gauge` | `kind`, `label`, `value`, `max`, `unit`, `color` |
| `ruler` | `steps` |
| `decision`, `decision_queue` | `session`, `free_text`, `limit` |
| `transcript`, `terminal` | `session` (default: `$item.id`, else `$selected.id`) |
| `composer` | `mode` (send, dispatch, newSession), `session`, `codebase`, `placeholder` |
| `settings_section` | none |
| effects and scenes | their parameters (section 4) and `fallback` |

- The app owns the Send button of a composer and the option buttons of a decision.
- `$selected` is the session of the Session or Focus screen, or of the open decision dialog.
- A `settings` layout without a `settings_section` is not used: Settings > Display must stay
  reachable (section 4). The app draws the compiled Settings screen in the place of the node.
- The `decision` screen is a dialog. The action `open` with `"screen": "decision"` and an `id`
  opens it over the screen on view. The app adds a Close button and the Esc key.
- `stop` and `autopilot` show a confirm that the app owns. The text is fixed and a theme
  cannot change it or press the button.
- The desktop renderer draws every chart kind and every gauge kind with plain boxes (line and
  area as columns, arc and dial as a bar). The effects and the scenes draw their `fallback`.
  `image` draws its `fallback`, or nothing.
- Type tokens: the nodes of a layout use `type.xs` to `type.xl` through the interface text size.
  The compiled Minimal screens keep their fixed sizes; they take the colors, the fonts and the
  radii of the theme, not the type sizes.

## Cuts in version 1
These parts of the ADR are not built. Each one needs its own decision.
- The three-way merge of `theme update`. The update works at file level: a file that the user
  edited stays, and the new upstream file lands as `<file>.upstream`.
- `aivoryd block add` (blocks into a theme). A user copies a file by hand.
- The error panel of `aivoryd theme dev` over the screen. `theme dev` prints each error.
- Install from `@<registry>/<name>`. `aivoryd registry build`, `add` and `list` work; the
  install takes `github:` sources and GitHub links only.
- The scenes (`city3d`, `fsn3d`, `radar`, `bloom`, `camera_wall`), the effects (`rain`,
  `scramble`, `scanlines`, `rings`) and `image` in the desktop renderer: they draw the fallback.
- The built-in themes as packages in `themes/` (section 11): the looks are still compiled.
- `phone`, `fold` and `watch` layouts: only the desktop renderer exists.

## Rejected alternatives
- **Looks as Rust code only (ADR 0022):** four display technologies make four builds per
  look, and users cannot add a look.
- **A script language in themes (Lua, JavaScript, WASM):** a script can read data, call the
  network or steer agents. Review of each theme is not possible. Data only is safe by design.
- **HTML and CSS themes in a web view:** the Watch and the native apps have no common web
  engine, and a web view has scripts.
- **TOML for layouts:** deep trees are hard to read and write in TOML.
- **A custom markup (KDL, XML-like):** each display needs its own parser, and IDEs give no
  completion without a plugin.
- **Install only, no copy (npm model):** the owner wants the shadcn model; users edit themes.

## Consequences
- The `LookKit` trait of ADR 0022 becomes the catalog renderer of the desktop app. The look
  code moves into catalog components and into the built-in packages.
- `Display.look: Look` (an enum) becomes a theme id (a string). Serde maps the old enum
  values to the built-in ids, so old settings files stay valid.
- The protocol gets the theme and registry types, the `theme/*` methods and two new schemas.
  The fixtures, the schemas and the phone copies need a bless.
- Each new component costs four renderers. The catalog must stay small.
- Third-party themes cannot reach the same detail as code. The scenes carry the hard parts.
- The website gets the docs section `docs/code` and the gallery page.

## Changes to other ADRs
- ADR 0022: section 4 (`LookKit`) and the rejected alternative "Looks as data files (JSON)";
  `Display.look` becomes a theme id; "The phone app keeps one look" ends.
- ADR 0023: the `theme/*` methods join the snapshot and the actions (S2); the open question
  "phone layouts per look" gets the display-class rule of section 1.

## Open questions
- The URL of the official `@aivory` registry: `https://aivory.net/registry/registry.json` or
  a GitHub repo.
- Text in layouts: one language, or a `strings` table per language?
- Must a phone layout exist for each built-in look, or does the phone use Minimal?

## Closed questions
- Paid themes: no. The registry has no paid items in this ADR. A paid marketplace needs its
  own decision: ADR 0025.

## Owner answers (2026-11-03)
- The look builds go on as compiled code now; each look is converted into a theme package after the
  component catalog exists.
- The official `@aivory` registry is a public GitHub repo with `registry.json`.
- Every built-in look gets its own phone layout.
- Layout text comes from a strings table per language.
- The registry may list paid themes; a license check is needed. Closed on 2026-11-07: no paid
  items here, see ADR 0025.
