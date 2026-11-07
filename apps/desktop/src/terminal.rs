//! "Open shell": a terminal window in a folder of this machine. Only the local daemon has such
//! a folder; the button is hidden for a remote daemon.

use std::process::{Command, Stdio};

/// The commands that open a terminal in `cwd`, in the order to try them.
pub fn terminal_commands(cwd: &str) -> Vec<(&'static str, Vec<String>)> {
    if cfg!(windows) {
        vec![
            // Windows Terminal, when it is installed.
            ("wt", vec!["-d".to_string(), cwd.to_string()]),
            // The console host: `start` opens a new window; /D sets its folder.
            (
                "cmd",
                ["/C", "start", "", "/D", cwd, "cmd", "/K"]
                    .map(str::to_string)
                    .to_vec(),
            ),
        ]
    } else if cfg!(target_os = "macos") {
        vec![(
            "open",
            vec!["-a".to_string(), "Terminal".to_string(), cwd.to_string()],
        )]
    } else {
        vec![
            (
                "x-terminal-emulator",
                vec![format!("--working-directory={cwd}")],
            ),
            ("gnome-terminal", vec![format!("--working-directory={cwd}")]),
            (
                "xterm",
                vec!["-e".to_string(), format!("cd '{cwd}' && exec $SHELL")],
            ),
        ]
    }
}

/// Open a terminal in `cwd`. `Err` names what failed.
pub fn open(cwd: &str) -> Result<(), String> {
    if cwd.trim().is_empty() {
        return Err("The daemon did not tell its folder.".to_string());
    }
    let mut last = String::from("No terminal program was found.");
    for (program, args) in terminal_commands(cwd) {
        match Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(e) => last = format!("{program}: {e}"),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_tries_the_terminal_app_first_then_the_console() {
        let commands = terminal_commands("C:\\work\\shop");
        assert_eq!(commands[0].0, "wt");
        assert_eq!(commands[0].1, ["-d", "C:\\work\\shop"]);
        assert_eq!(commands[1].0, "cmd");
        let args = commands[1].1.join(" ");
        assert!(args.contains("/D C:\\work\\shop"));
        assert!(args.ends_with("cmd /K"));
    }

    #[test]
    fn every_candidate_gets_the_folder() {
        for (_, args) in terminal_commands("/srv/app") {
            assert!(args.iter().any(|a| a.contains("/srv/app")));
        }
    }

    #[test]
    fn an_empty_folder_is_refused() {
        assert!(open("  ").is_err());
    }
}
