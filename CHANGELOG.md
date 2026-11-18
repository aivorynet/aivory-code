# Changelog

## 2026.2.2

- Bridge session screen on a large window: the chat scroll bar stays in its track and no longer
  runs behind Attach and Send.
- Plan, Edit, Auto and the model pickers: the pills grow with the text size, so large text stays
  inside them.
- Settings > Start with the system: the help text wraps, and the switch keeps its place.

## 2026.2.1

- Pair a phone from every look: a "Pair a phone" key in the settings of Bridge, Holo, Neon, Operator,
  Tactical and Command opens the pairing card with a fresh code. Settings > "Devices and pairing"
  shows the pairing card first.
- First start: the window says "Starting aivoryd…" while the app starts the daemon, without error
  messages, and connects within a second.
- The coffee mug of aivory.net/coffee next to "Buy Paul a coffee".

## 2026.2.0

- Start with the system: a switch in Settings > Daemons and devices starts aivoryd when you sign in
  (Task Scheduler, LaunchAgent, or systemd). The app still starts aivoryd when it finds none.
- Microsoft Store package: the daemon has a fixed path (app execution alias), so the sign-in task
  and scheduled tasks keep working after a Store update.
- Machine mesh: daemons on your other computers link up, share sessions, and sync files.
- Theme sync between the desktop and the phone.
- Reworked looks: Operator, Bridge, Neon, Holo, Tactical, Command, Navigator, and Minimal fill wide
  screens, and each look has its own session, inbox, and settings screens.

## 2026.1.0 (first public version)

- A control room for Claude Code, Codex CLI, and Gemini CLI sessions: desktop app, daemon, phone app.
- Eight looks: Minimal, Bridge, Operator, Neon, Navigator, Holo, Tactical, Command.
- Theme packages: install a theme from a GitHub link.
- Free for everyone under the MIT license.
