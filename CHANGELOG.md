# Changelog

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
