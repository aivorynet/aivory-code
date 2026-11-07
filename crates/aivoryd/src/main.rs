use std::io::{self, BufWriter, Write};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use aivory_sessions::{Provider, SessionSummary, timeutil};
use aivoryd::rpc::ListFilter;
use aivoryd::{Config, Daemon, Endpoint, ResumeParams, Server, client};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Parser, Debug)]
#[command(
    name = "aivoryd",
    version,
    about = "AIVory Code daemon: list and resume native Claude, Codex, and Gemini sessions"
)]
struct Cli {
    /// Claude home directory (default: ~/.claude)
    #[arg(long, global = true, env = "AIVORY_CLAUDE_HOME", value_name = "DIR")]
    claude_home: Option<PathBuf>,
    /// Codex home directory (default: ~/.codex)
    #[arg(long, global = true, env = "AIVORY_CODEX_HOME", value_name = "DIR")]
    codex_home: Option<PathBuf>,
    /// Gemini home directory (default: ~/.gemini)
    #[arg(long, global = true, env = "AIVORY_GEMINI_HOME", value_name = "DIR")]
    gemini_home: Option<PathBuf>,
    /// Scan cache file (default: per-user cache directory)
    #[arg(long, global = true, value_name = "FILE", conflicts_with = "no_cache")]
    cache_file: Option<PathBuf>,
    /// Do not read or write the scan cache
    #[arg(long, global = true)]
    no_cache: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List the native sessions on this machine, newest first
    List {
        #[arg(long)]
        provider: Option<Provider>,
        /// Only sessions in this directory or below it
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Print one session as JSON
    Show {
        /// Session id or a unique id prefix
        id: String,
        #[arg(long)]
        provider: Option<Provider>,
    },
    /// Resume a session with the native CLI and print the normalised events as JSON lines
    Resume {
        /// Session id or a unique id prefix
        id: String,
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        provider: Option<Provider>,
        #[arg(long)]
        model: Option<String>,
        /// Print events that the parser does not map
        #[arg(long)]
        raw: bool,
        /// Extra argument for the CLI (repeatable), for example --extra-arg=--permission-mode=acceptEdits
        #[arg(long = "extra-arg", value_name = "ARG", allow_hyphen_values = true)]
        extra_args: Vec<String>,
    },
    /// Serve JSON-RPC 2.0 (newline-delimited JSON) on a named pipe or Unix socket
    Serve {
        #[arg(long, default_value = Endpoint::DEFAULT_NAME)]
        endpoint: String,
        /// Serve the war room on the demo data of the design (no real sessions)
        #[arg(long)]
        demo: bool,
        /// Show only sessions whose cwd lies below this folder
        #[arg(long)]
        scope: Option<String>,
        /// Serve the phone app over WebSocket on this address, for example 0.0.0.0:7420
        #[arg(long)]
        remote: Option<std::net::SocketAddr>,
        /// Serve the phone app over iroh (QUIC with hole punching and a relay), also outside
        /// the LAN. Can run next to --remote and --demo
        #[arg(long)]
        iroh: bool,
        /// Relay for --iroh: a URL (setting remote.relay), `default` for the iroh relays, or
        /// `none` for direct paths only
        #[arg(long, env = "AIVORY_RELAY", value_name = "URL")]
        relay: Option<String>,
        /// URL of the push gateway for the sealed phone push, for example
        /// https://push.example.com/v1/push (setting remote.pushUrl). No URL: no push
        #[arg(long, env = "AIVORY_PUSH_URL", value_name = "URL")]
        push_url: Option<String>,
    },
    /// The sealed phone push
    Push {
        #[command(subcommand)]
        action: PushCommand,
    },
    /// Call one JSON-RPC method of a running daemon and print the messages
    Call {
        method: String,
        /// JSON parameters, for example '{"provider":"codex"}'
        #[arg(long)]
        params: Option<String>,
        #[arg(long, default_value = Endpoint::DEFAULT_NAME)]
        endpoint: String,
    },
    /// Theme packages: init, add, list, diff, update, remove, check, pack, dev
    Theme {
        #[command(subcommand)]
        action: aivoryd::theme::cli::ThemeCommand,
        #[arg(long, default_value = Endpoint::DEFAULT_NAME)]
        endpoint: String,
    },
    /// Theme registries: build, add, list, remove
    Registry {
        #[command(subcommand)]
        action: aivoryd::theme::cli::RegistryCommand,
        #[arg(long, default_value = Endpoint::DEFAULT_NAME)]
        endpoint: String,
    },
    /// Write or print the part map of a repository (.aivory/map.toml)
    Map {
        #[command(subcommand)]
        action: MapCommand,
    },
}

#[derive(Subcommand, Debug)]
enum PushCommand {
    /// Print the SHA-256 of the daemon key for the allow list of the push gateway. Creates the
    /// key file (push.key, next to iroh.key) when it is missing
    Key,
}

#[derive(Subcommand, Debug)]
enum MapCommand {
    /// Guess a first .aivory/map.toml from the top folders of a repository
    Init {
        /// Repository root
        repo: PathBuf,
        /// Replace an existing map.toml
        #[arg(long)]
        force: bool,
    },
    /// Print the parts of a repository map as JSON
    Show {
        /// Repository root
        repo: PathBuf,
    },
}

/// The probe of the real Claude limits, when the `claude` CLI is installed.
/// `AIVORY_USAGE_PROBE=0` turns it off.
fn usage_probe() -> Option<aivoryd::claude_usage::Shared> {
    if std::env::var("AIVORY_USAGE_PROBE").is_ok_and(|v| v == "0") {
        return None;
    }
    let program = aivory_agents::resolve_program(std::path::Path::new("claude")).ok()?;
    let cwd = std::env::temp_dir().join("aivory-usage-probe");
    Some(aivoryd::claude_usage::spawn(program, cwd))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("aivoryd: {message}");
            ExitCode::from(2)
        }
    }
}

fn build_config(cli: &Cli) -> Result<Config, String> {
    let mut config = Config::from_environment()?;
    if let Some(p) = &cli.claude_home {
        config.roots.claude.clone_from(p);
    }
    if let Some(p) = &cli.codex_home {
        config.roots.codex.clone_from(p);
    }
    if let Some(p) = &cli.gemini_home {
        config.roots.gemini.clone_from(p);
    }
    if cli.no_cache {
        config.cache_file = None;
    } else if let Some(p) = &cli.cache_file {
        config.cache_file = Some(p.clone());
    }
    Ok(config)
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    // `call` needs no configuration; everything else does.
    if let Command::Call {
        method,
        params,
        endpoint,
    } = &cli.command
    {
        return call(method, params.as_deref(), endpoint);
    }
    if let Command::Map { action } = &cli.command {
        return map(action);
    }
    let config = build_config(&cli)?;
    match &cli.command {
        Command::Theme { action, endpoint } => {
            return aivoryd::theme::cli::run_theme(action, endpoint, config.cache_file.as_deref());
        }
        Command::Registry { action, endpoint } => {
            return aivoryd::theme::cli::run_registry(
                action,
                endpoint,
                config.cache_file.as_deref(),
            );
        }
        _ => {}
    }
    let data_dir = config
        .cache_file
        .as_ref()
        .and_then(|f| f.parent())
        .map(std::path::Path::to_path_buf);
    let store = data_dir.as_ref().map(|dir| dir.join("aivory.db"));
    if let Command::Push {
        action: PushCommand::Key,
    } = &cli.command
    {
        let dir = data_dir.as_ref().ok_or("no data folder for the key file")?;
        let file = aivoryd::push::key_file(dir);
        let key = aivoryd::push::load_or_create_key(&file)
            .map_err(|e| format!("cannot use {}: {e}", file.display()))?;
        println!("{}", aivoryd::push::key_hash(&key));
        return Ok(ExitCode::SUCCESS);
    }
    let pairings = match &cli.command {
        Command::Serve { remote, iroh, .. } if remote.is_some() || *iroh => {
            Some(Arc::new(aivoryd::remote::Pairings::load_with(
                data_dir.as_ref().map(|dir| dir.join("devices.json")),
                remote.map(aivoryd::remote::public_addr),
            )))
        }
        _ => None,
    };
    let codex_home = config.roots.codex.clone();
    let daemon = match &cli.command {
        Command::Serve { demo: true, .. } => match &pairings {
            Some(p) => Daemon::with_demo(config).with_pairings(Arc::clone(p)),
            None => Daemon::with_demo(config),
        },
        Command::Serve { scope, .. } => Daemon::with_live(
            config,
            aivoryd::live::LiveOptions {
                scope: scope.clone(),
                store: store.clone(),
                pairings: pairings.clone(),
                usage_probe: usage_probe(),
                codex_home: Some(codex_home.clone()),
            },
        ),
        _ => Daemon::new(config),
    };
    match cli.command {
        Command::List {
            provider,
            cwd,
            limit,
            json,
        } => {
            let (sessions, _) = daemon.list(&ListFilter {
                provider,
                cwd,
                limit,
            });
            let mut out = BufWriter::new(io::stdout().lock());
            if json {
                serde_json::to_writer(&mut out, &sessions).map_err(|e| e.to_string())?;
                writeln!(out).map_err(|e| e.to_string())?;
            } else {
                print_table(&mut out, &sessions).map_err(|e| e.to_string())?;
            }
            out.flush().map_err(|e| e.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Show { id, provider } => {
            let session = daemon.get(&id, provider).map_err(|e| e.message)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Resume {
            id,
            prompt,
            provider,
            model,
            raw,
            extra_args,
        } => {
            let params = ResumeParams {
                id,
                prompt,
                provider,
                model,
                extra_args,
                include_raw: raw,
            };
            let mut out = io::stdout().lock();
            let (_, outcome) = daemon
                .resume(&params, &mut |_, _, event| match writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&event).unwrap_or_default()
                )
                .and_then(|()| out.flush())
                {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(_) => ControlFlow::Break(()),
                })
                .map_err(|e| e.message)?;
            Ok(if outcome.completed {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Serve {
            endpoint,
            demo,
            remote,
            iroh,
            relay,
            push_url,
            ..
        } => {
            let endpoint = Endpoint::new(&endpoint).map_err(|e| e.to_string())?;
            let daemon = Arc::new(daemon);
            // A process that `daemon/restart` started waits for the old one to let go.
            if let (Some(addr), Some(pairings)) = (remote, pairings.clone()) {
                aivoryd::actions::retry_bind(|| {
                    aivoryd::remote::serve(Arc::clone(&daemon), Arc::clone(&pairings), addr)
                })
                .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
            }
            // Keep the link alive until the server ends.
            let _iroh = match (iroh, pairings.clone()) {
                (true, Some(pairings)) => {
                    let options = aivoryd::iroh_link::IrohOptions {
                        secret_file: data_dir.as_ref().map(|dir| dir.join("iroh.key")),
                        relay: aivory_link::endpoint::RelayChoice::from_setting(
                            relay
                                .clone()
                                .or_else(|| aivoryd::live::saved_relay(store.as_deref()))
                                .as_deref(),
                        ),
                        bind: None,
                    };
                    let link =
                        aivoryd::iroh_link::IrohLink::start(Arc::clone(&daemon), pairings, options)
                            .map_err(|e| format!("cannot start the iroh link: {e}"))?;
                    eprintln!("aivoryd iroh link on node {}", link.node_id());
                    Some(link)
                }
                _ => None,
            };
            // Keep the push worker alive until the server ends.
            let _push = match pairings.clone() {
                Some(pairings) => Some(start_push(
                    Arc::clone(&daemon),
                    pairings,
                    data_dir.as_deref(),
                    push_url,
                )?),
                None => None,
            };
            let restart = Arc::clone(&daemon);
            let server = aivoryd::actions::retry_bind(|| {
                Server::bind(Arc::clone(&daemon), endpoint.clone())
            })
            .map_err(|e| format!("cannot listen on {}: {e}", endpoint.address()))?;
            eprintln!(
                "aivoryd {} listening on {}{}",
                env!("CARGO_PKG_VERSION"),
                endpoint.address(),
                if demo { " (demo data)" } else { "" }
            );
            server.run().map_err(|e| e.to_string())?;
            if restart.is_restart_requested() {
                // Let the response of `daemon/restart` reach the client, then hand over.
                std::thread::sleep(std::time::Duration::from_millis(300));
                aivoryd::actions::spawn_restart()
                    .map_err(|e| format!("cannot start the new daemon: {e}"))?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Call { .. }
        | Command::Map { .. }
        | Command::Push { .. }
        | Command::Theme { .. }
        | Command::Registry { .. } => {
            unreachable!("handled above")
        }
    }
}

/// Start the push worker for the paired phones. It sends nothing without a gateway URL.
fn start_push(
    daemon: Arc<Daemon>,
    pairings: Arc<aivoryd::remote::Pairings>,
    data_dir: Option<&std::path::Path>,
    url: Option<String>,
) -> Result<aivoryd::push::PushService, String> {
    let key = match data_dir {
        Some(dir) => aivoryd::push::load_or_create_key(&aivoryd::push::key_file(dir))
            .map_err(|e| format!("cannot use the push key file: {e}"))?,
        None => aivoryd::push::random_key(),
    };
    let sender = Arc::new(aivoryd::push::HttpSender::new(&key));
    aivoryd::push::PushService::start(
        daemon,
        pairings,
        sender,
        aivoryd::push::Options {
            url,
            ..Default::default()
        },
    )
    .map_err(|e| format!("cannot start the push worker: {e}"))
}

fn map(action: &MapCommand) -> Result<ExitCode, String> {
    match action {
        MapCommand::Init { repo, force } => {
            let file = aivory_map::init(repo);
            let path = aivory_map::write(repo, &file, *force).map_err(|e| match e {
                aivory_map::MapError::Exists { path } => {
                    format!(
                        "{} already exists; use --force to replace it",
                        path.display()
                    )
                }
                other => other.to_string(),
            })?;
            println!("wrote {} ({} parts)", path.display(), file.parts.len());
        }
        MapCommand::Show { repo } => {
            let file = aivory_map::load(repo).map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&file.parts).map_err(|e| e.to_string())?
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn call(method: &str, params: Option<&str>, endpoint: &str) -> Result<ExitCode, String> {
    let endpoint = Endpoint::new(endpoint).map_err(|e| e.to_string())?;
    let params: Value = match params {
        Some(text) => {
            serde_json::from_str(text).map_err(|e| format!("--params is not valid JSON: {e}"))?
        }
        None => json!({}),
    };
    let response = client::call(&endpoint, method, params, &mut |note| println!("{note}"))
        .map_err(|e| format!("cannot reach the daemon at {}: {e}", endpoint.address()))?;
    println!("{response}");
    Ok(if response.get("error").is_some() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn print_table(out: &mut impl Write, sessions: &[SessionSummary]) -> io::Result<()> {
    for s in sessions {
        let id: String = s.id.chars().take(8).collect();
        writeln!(
            out,
            "{:<7} {:<8} {} {} {}",
            s.provider,
            id,
            timeutil::format_utc_minutes(s.updated),
            s.cwd.as_deref().unwrap_or("-"),
            s.title.as_deref().unwrap_or("")
        )?;
    }
    Ok(())
}
