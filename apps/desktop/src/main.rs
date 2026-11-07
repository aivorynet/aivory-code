//! AIVory Code desktop app (GPUI). It talks to `aivoryd` over the daemon endpoint.
//! `aivory --demo` serves the demo data of the design in this process.

// No console window, also in debug builds; aivoryd keeps the logs.
#![windows_subsystem = "windows"]
// GPUI views hold entity and task handles without a Debug impl.
#![allow(missing_debug_implementations)]

mod actions;
mod assets;
mod block;
mod bridge_focus;
mod bridge_kit;
mod bridge_lens;
mod bridge_screens;
mod command_inbox;
mod command_kit;
mod command_screens;
mod compose;
mod console;
mod daemon;
mod focus;
mod forms;
mod holo_focus;
mod holo_kit;
mod holo_lens;
mod holo_screens;
mod inbox;
mod lens;
mod look;
mod model;
mod new_session;
mod notice;
mod radar;
mod repos;
mod rts;
mod session;
mod settings;
mod shell;
mod speech;
mod tactical_focus;
mod tactical_kit;
mod tactical_lens;
mod tactical_screens;
mod tasks;
mod terminal;
mod theme;
mod themed;
mod ui;
mod vtext;
mod warroom;

use std::process::ExitCode;

use aivoryd::Endpoint;
use clap::Parser;
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, point, px, size};

#[derive(Parser, Debug)]
#[command(name = "aivory", version, about = "AIVory Code desktop app")]
struct Cli {
    /// The daemon endpoint name
    #[arg(long, default_value = Endpoint::DEFAULT_NAME)]
    endpoint: String,
    /// Serve the demo data of the design in this process (no daemon needed)
    #[arg(long)]
    demo: bool,
    /// Copy live sessions until this many are live (performance checks)
    #[arg(long, default_value_t = 0)]
    stress: usize,
    /// Redraw every frame and log the frame rate to stderr
    #[arg(long)]
    fps: bool,
    /// Open the window without focus, outside all monitors (screenshot runs that must not
    /// disturb the user). Use it with `--demo`.
    #[arg(long)]
    offscreen: bool,
    /// With `--demo`: show sample transcripts with markdown, diffs, colors and todos
    #[arg(long)]
    demo_rich: bool,
    /// Scroll the war room to this row at start (1 is the first group of panes; screenshot runs)
    #[arg(long)]
    scroll_row: Option<usize>,
}

/// The top left corner of the `--offscreen` window: no monitor reaches it.
const OFFSCREEN_ORIGIN: f32 = -20_000.;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let endpoint = match if cli.demo {
        daemon::start_demo()
    } else {
        Endpoint::new(&cli.endpoint).map_err(anyhow::Error::from)
    } {
        Ok(endpoint) => endpoint,
        Err(e) => {
            eprintln!("aivory: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            if let Err(e) = assets::load_fonts(cx) {
                eprintln!("aivory: cannot load the bundled fonts: {e:#}");
            }
            // The text fields of the composers.
            gpui_base::init(cx);
            // `AIVORY_WINDOW=900x640`: another window size for screenshot runs.
            let (w, h) = std::env::var("AIVORY_WINDOW")
                .ok()
                .and_then(|v| {
                    let (w, h) = v.split_once('x')?;
                    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
                })
                .unwrap_or((1440., 900.));
            let bounds = if cli.offscreen {
                Bounds::new(
                    point(px(OFFSCREEN_ORIGIN), px(OFFSCREEN_ORIGIN)),
                    size(px(w), px(h)),
                )
            } else {
                Bounds::centered(None, size(px(w), px(h)), cx)
            };
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: !cli.offscreen,
                window_min_size: Some(size(px(360.), px(640.))),
                app_id: Some("aivory-code".into()),
                // `--fps` measures the real frame rate, also while the window is not focused.
                inactive_frame_interval: if cli.fps {
                    None
                } else {
                    WindowOptions::default().inactive_frame_interval
                },
                ..Default::default()
            };
            cx.open_window(options, |_, cx| {
                let model = cx.new(|cx| {
                    model::Model::new(
                        endpoint,
                        cli.stress,
                        cli.fps,
                        cli.demo_rich,
                        cli.scroll_row,
                        cx,
                    )
                });
                cx.new(|cx| shell::Shell::new(model, cx))
            })
            .expect("cannot open the main window");
            if !cli.offscreen {
                cx.activate(true);
            }
        });
    ExitCode::SUCCESS
}
