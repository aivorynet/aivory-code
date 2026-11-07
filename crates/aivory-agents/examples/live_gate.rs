//! Manual gate for `LiveRun`: one real Claude session in a throwaway folder.
//!
//! ```text
//! cargo run -p aivory-agents --example live_gate -- <cwd> <prompt> [options]
//!   --mode <permission mode>     default: default
//!   --model <model>              default: haiku
//!   --answer allow|always|deny[:<message>]|choice:<text>
//!                                how to answer the next ask; repeat it for more asks (the last repeats)
//!   --then <text>                another turn, sent after the turn before it ended; repeatable
//!   --session-id <uuid>          id of the new session
//!   --resume <id>                continue a native session (the prompt is the next turn)
//!   --interrupt-on-ask           interrupt the turn when the first ask arrives
//!   --interrupt-after-tool <s>   interrupt 4 s after a tool line that contains <s>
//! ```
//!
//! It prints every event. It kills the process at the end. A real run costs subscription use,
//! so use a cheap model and a one-line prompt. Set `AIVORY_LIVE_TRACE=<file>` for the raw lines.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use aivory_agents::live::{LiveRun, RunEvent, StartOptions};
use aivory_agents::mcp::Answer;
use aivory_protocol::LineKind;

fn main() {
    let mut args = std::env::args().skip(1);
    let cwd = PathBuf::from(args.next().expect("cwd"));
    let prompt = args.next().expect("prompt");
    let mut mode = "default".to_string();
    let mut model = "haiku".to_string();
    let mut answers: VecDeque<String> = VecDeque::new();
    let mut thens: VecDeque<String> = VecDeque::new();
    let mut interrupted_at: Option<Instant> = None;
    let mut session_id = None;
    let mut resume = None;
    let mut interrupt_on_ask = false;
    let mut interrupt_after_tool: Option<String> = None;
    let mut scheduled: Option<Instant> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--mode" => mode = args.next().expect("mode"),
            "--model" => model = args.next().expect("model"),
            "--answer" => answers.push_back(args.next().expect("answer")),
            "--then" => thens.push_back(args.next().expect("text")),
            "--session-id" => session_id = args.next(),
            "--resume" => resume = args.next(),
            "--interrupt-on-ask" => interrupt_on_ask = true,
            "--interrupt-after-tool" => interrupt_after_tool = args.next(),
            other => panic!("unknown option {other}"),
        }
    }
    let (run, rx) = LiveRun::start(StartOptions {
        cwd,
        prompt,
        model: Some(model),
        permission_mode: Some(mode),
        binary: None,
        session_id,
        resume,
        ..StartOptions::default()
    })
    .expect("start");
    let begin = Instant::now();
    let deadline = Duration::from_secs(240);
    let mut turns = 0;
    let mut asks = 0;
    loop {
        if scheduled.is_some_and(|t| Instant::now() >= t) {
            println!("-> interrupt (scheduled)");
            run.interrupt().expect("interrupt");
            scheduled = None;
            interrupted_at = Some(Instant::now());
        }
        let event = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => {
                if interrupted_at.is_some_and(|t| t.elapsed() > Duration::from_secs(25)) {
                    println!("NO EVENT 25 s AFTER THE INTERRUPT");
                    run.kill();
                    interrupted_at = None;
                }
                if begin.elapsed() > deadline {
                    println!("TIMEOUT");
                    run.kill();
                    break;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        println!(
            "[{:>5.1}s {:?}] {event:?}",
            begin.elapsed().as_secs_f32(),
            run.state()
        );
        match event {
            RunEvent::Line(l)
                if l.kind == LineKind::Tool
                    && interrupt_after_tool
                        .as_ref()
                        .is_some_and(|s| l.text.contains(s.as_str())) =>
            {
                scheduled = Some(Instant::now() + Duration::from_secs(4));
            }
            RunEvent::Ask(ask) => {
                asks += 1;
                if interrupt_on_ask && asks == 1 {
                    println!("-> interrupt");
                    run.interrupt().expect("interrupt");
                    interrupted_at = Some(Instant::now());
                    continue;
                }
                let spec = if answers.len() > 1 {
                    answers.pop_front().unwrap_or_default()
                } else {
                    answers.front().cloned().unwrap_or_else(|| "allow".into())
                };
                let a = match spec.as_str() {
                    "allow" => Answer::allow(),
                    "always" => Answer::allow_always(),
                    "deny" => Answer::deny("Denied by the gate."),
                    s if s.starts_with("deny:") => Answer::deny(&s[5..]),
                    s => Answer::choice(s.strip_prefix("choice:").unwrap_or(s)),
                };
                println!("-> answer {a:?}");
                run.answer(&ask.id, a).expect("answer");
            }
            RunEvent::TurnEnded { .. } => {
                turns += 1;
                interrupted_at = None;
                match thens.pop_front() {
                    Some(text) => {
                        println!("-> send {text:?}");
                        run.send(&text).expect("send");
                    }
                    None => {
                        println!("-> kill after {turns} turn(s)");
                        run.kill();
                    }
                }
            }
            RunEvent::Exited { .. } => break,
            _ => {}
        }
    }
    println!("done: state {:?}", run.state());
}
