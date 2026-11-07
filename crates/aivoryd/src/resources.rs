//! `resources/memory`: the RAM of the machine and of the agent processes, and the prompt of
//! the organizer agent (`organizer/start`).

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aivory_protocol::{AgentMem, MemSample, Memory, OrganizerMode};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// One sample each 5 s, 30 minutes.
const EVERY: Duration = Duration::from_secs(5);
const KEEP: usize = 360;
const TOP: usize = 6;
const MB: u64 = 1024 * 1024;

/// The programs that count as agents.
const AGENTS: [&str; 4] = ["claude", "codex", "gemini", "node"];

pub struct Sampler {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler").finish_non_exhaustive()
    }
}

struct Inner {
    sys: System,
    last: Option<Instant>,
    samples: VecDeque<MemSample>,
    now: Memory,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                sys: System::new(),
                last: None,
                samples: VecDeque::new(),
                now: Memory::default(),
            }),
        }
    }
}

/// `claude.exe` → `claude`; other programs → `None`.
pub fn agent_name(program: &str) -> Option<&'static str> {
    let stem = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let stem = stem.trim_end_matches(".exe");
    AGENTS.iter().copied().find(|a| *a == stem)
}

impl Sampler {
    /// The current view; a new sample when the last one is older than [`EVERY`].
    pub fn memory(&self, now_ms: i64) -> Memory {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if s.last.is_none_or(|t| t.elapsed() >= EVERY) {
            s.last = Some(Instant::now());
            s.sys.refresh_memory();
            s.sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                ProcessRefreshKind::new().with_memory(),
            );
            let mut top: Vec<AgentMem> = s
                .sys
                .processes()
                .iter()
                .filter_map(|(pid, p)| {
                    let name = agent_name(&p.name().to_string_lossy())?;
                    Some(AgentMem {
                        pid: pid.as_u32(),
                        name: name.to_string(),
                        mb: (p.memory() / MB) as u32,
                    })
                })
                .collect();
            top.sort_by(|a, b| b.mb.cmp(&a.mb).then(a.pid.cmp(&b.pid)));
            let agents_mb = top.iter().map(|a| a.mb).sum();
            top.truncate(TOP);
            let total_mb = (s.sys.total_memory() / MB) as u32;
            let used_mb = (s.sys.used_memory() / MB) as u32;
            s.samples.push_back(MemSample {
                at: now_ms,
                used_mb,
                agents_mb,
            });
            while s.samples.len() > KEEP {
                s.samples.pop_front();
            }
            let samples = s.samples.iter().copied().collect();
            s.now = Memory {
                total_mb,
                used_mb,
                agents_mb,
                samples,
                top,
            };
        }
        s.now.clone()
    }
}

/// The first message of the organizer agent. `aivoryd` is the path of this program, so the
/// agent calls the daemon with `<aivoryd> call <method> --params <json>`.
pub fn organizer_prompt(mode: OrganizerMode, aivoryd: &str) -> String {
    let call = format!("\"{aivoryd}\" call");
    let watch = match mode {
        OrganizerMode::FullAuto | OrganizerMode::MemoryAndSessions => {
            "the RAM of the machine and of the agents, and the state of every session"
        }
        OrganizerMode::FeedbackOnly => "the state of every session (RAM is not your job)",
        OrganizerMode::MemoryOnly => "the RAM of the machine and of the agents only",
    };
    let act = match mode {
        OrganizerMode::FullAuto => format!(
            "You may act. Allowed: `{call} session/pause|session/park|session/resume|session/stop --params '{{\"id\":\"<id>\"}}'` \
for sessions that aivoryd runs (`controlled: true`), `{call} autopilot/set --params '{{...}}'` to change the slots, \
and `{call} tasks/create` to queue work. Never stop a session that waits for the user or works on a decision; \
prefer park over stop. Free RAM first by parking idle sessions, the oldest first. Say what you did and why in one line each."
        ),
        _ => "You do not act on sessions. You give feedback only: one short line per finding, \
with the session id and what the user should do. When the user must decide, ask the user."
            .to_string(),
    };
    format!(
        "You are the organizer of the AIVory Code war room. Mode: {label}. You watch {watch}.\n\
Read the state with `{call} warroom/snapshot --params '{{}}'` (sessions, states, codebases, autopilot) \
and `{call} resources/memory --params '{{}}'` (totalMb, usedMb, agentsMb, top processes).\n\
{act}\n\
Rules: never print the message content of a session; use ids, titles, states and numbers only. \
Free RAM under 15% of total is low; under 8% is critical. \
Loop: check, report or act, then wait with `sleep 120`, and repeat until the user stops you. \
Keep each report under 6 lines.",
        label = mode.label(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_programs_are_known() {
        assert_eq!(agent_name("claude.exe"), Some("claude"));
        assert_eq!(agent_name("C:\\x\\Codex.EXE"), Some("codex"));
        assert_eq!(agent_name("/usr/bin/gemini"), Some("gemini"));
        assert_eq!(agent_name("chrome.exe"), None);
    }

    #[test]
    fn the_prompt_follows_the_mode() {
        let full = organizer_prompt(OrganizerMode::FullAuto, "aivoryd");
        assert!(full.contains("session/park"));
        let only = organizer_prompt(OrganizerMode::MemoryOnly, "aivoryd");
        assert!(!only.contains("session/park"));
        assert!(only.contains("RAM"));
    }

    #[test]
    fn the_sampler_keeps_a_history() {
        let s = Sampler::default();
        let m = s.memory(1);
        assert!(m.total_mb > 0);
        assert_eq!(m.samples.len(), 1);
        // A second call within 5 s returns the same view.
        assert_eq!(s.memory(2).samples.len(), 1);
    }
}
