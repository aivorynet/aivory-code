//! Start the daemon with the system: one entry in the scheduler of the operating system that
//! runs `aivoryd serve --iroh --endpoint <name>` when the user signs in (setting
//! `startWithSystem`). Windows: a Task Scheduler task with a logon trigger. macOS: a
//! LaunchAgent with `RunAtLoad`. Linux: a systemd user unit, or an `@reboot` crontab line.
//!
//! The desktop app starts the daemon with the same arguments when it finds none, so both ways
//! give one daemon with the phone link on.
//!
//! In the Microsoft Store package the binaries live below `WindowsApps` in a folder that
//! changes with each update. The entries then use the app execution alias
//! (`%LOCALAPPDATA%\Microsoft\WindowsApps\aivoryd.exe`), which stays the same.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::schedule::{
    OsScheduler, crontab_merge, run, run_out, systemd_dir, utf16_bom, xml_escape,
};

/// The arguments that start the daemon in the background. The app and the entries use them.
pub fn serve_args(endpoint: &str) -> Vec<String> {
    ["serve", "--iroh", "--endpoint", endpoint]
        .into_iter()
        .map(str::to_string)
        .collect()
}

static ENDPOINT: OnceLock<String> = OnceLock::new();

/// Note the endpoint name that this daemon serves. The entry starts the daemon on it.
pub fn set_endpoint(name: &str) {
    let _ = ENDPOINT.set(name.to_string());
}

fn endpoint() -> &'static str {
    ENDPOINT
        .get()
        .map_or(crate::Endpoint::DEFAULT_NAME, String::as_str)
}

/// A path to `exe` that stays valid over an update. Outside a Store package: `exe` itself.
pub fn stable_exe(exe: &Path) -> PathBuf {
    let packaged = exe
        .components()
        .any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps"));
    if cfg!(windows)
        && packaged
        && let (Some(local), Some(name)) = (std::env::var_os("LOCALAPPDATA"), exe.file_name())
    {
        let alias = PathBuf::from(local)
            .join("Microsoft")
            .join("WindowsApps")
            .join(name);
        // An alias is a reparse point that only the shell follows: test the link itself.
        if fs::symlink_metadata(&alias).is_ok() {
            return alias;
        }
    }
    exe.to_path_buf()
}

/// The daemon binary for the entries: this process, by its stable path.
pub fn daemon_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aivoryd"));
    stable_exe(&exe)
}

const WINDOWS_TASK: &str = "AIVory\\aivoryd-autostart";
const LAUNCHD_LABEL: &str = "net.aivory.daemon";
const SYSTEMD_UNIT: &str = "aivory-daemon.service";
const CRON_ID: &str = "daemon-autostart";

/// Write (`on`) or remove the entry. Writing replaces an old entry, so a moved or updated
/// install heals when the daemon writes it again at start.
pub fn apply(on: bool) -> Result<(), String> {
    apply_with(&OsScheduler::detect(), on, &daemon_exe(), endpoint())
}

pub fn apply_with(os: &OsScheduler, on: bool, exe: &Path, endpoint: &str) -> Result<(), String> {
    let args = serve_args(endpoint);
    match (os, on) {
        (OsScheduler::Windows, true) => {
            let file = std::env::temp_dir().join("aivory-autostart.xml");
            fs::write(&file, utf16_bom(&windows_task_xml(exe, &args, &user_id())))
                .map_err(|e| e.to_string())?;
            let r = run(
                "schtasks",
                &[
                    "/Create",
                    "/TN",
                    WINDOWS_TASK,
                    "/XML",
                    &file.to_string_lossy(),
                    "/F",
                ],
                None,
            );
            let _ = fs::remove_file(&file);
            r
        }
        (OsScheduler::Windows, false) => {
            if run_out("schtasks", &["/Query", "/TN", WINDOWS_TASK]).is_err() {
                return Ok(());
            }
            run("schtasks", &["/Delete", "/TN", WINDOWS_TASK, "/F"], None)
        }
        (OsScheduler::Launchd, on) => {
            let path = std::env::home_dir()
                .ok_or("no home folder")?
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LAUNCHD_LABEL}.plist"));
            if on {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                // launchd loads the file at the next sign-in; the daemon runs now already.
                fs::write(&path, launchd_plist(exe, &args)).map_err(|e| e.to_string())
            } else {
                match fs::remove_file(&path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                    _ => Ok(()),
                }
            }
        }
        (OsScheduler::Systemd, on) => {
            let dir = systemd_dir().ok_or("no home folder")?;
            let path = dir.join(SYSTEMD_UNIT);
            if on {
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                fs::write(&path, systemd_unit(exe, &args)).map_err(|e| e.to_string())?;
                run("systemctl", &["--user", "daemon-reload"], None)?;
                // Enable only: the daemon runs now already.
                run("systemctl", &["--user", "enable", SYSTEMD_UNIT], None)
            } else {
                if path.exists() {
                    let _ = run("systemctl", &["--user", "disable", SYSTEMD_UNIT], None);
                    fs::remove_file(&path).map_err(|e| e.to_string())?;
                    let _ = run("systemctl", &["--user", "daemon-reload"], None);
                }
                Ok(())
            }
        }
        (OsScheduler::Crontab, on) => {
            let old = run_out("crontab", &["-l"]).unwrap_or_default();
            let line = on.then(|| format!("@reboot {}", shell_line(exe, &args)));
            run(
                "crontab",
                &["-"],
                Some(&crontab_merge(&old, CRON_ID, line.as_deref())),
            )
        }
        (OsScheduler::Fake(dir), on) => {
            let file = dir.join("autostart.txt");
            if on {
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                fs::write(&file, shell_line(exe, &args)).map_err(|e| e.to_string())
            } else {
                let _ = fs::remove_file(&file);
                Ok(())
            }
        }
        (OsScheduler::None, _) => Ok(()),
    }
}

/// `DOMAIN\user` of the signed-in user: the logon trigger fires for this user only, so the
/// task needs no administrator rights.
fn user_id() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    }
}

fn quote_windows(arg: &str) -> String {
    if arg.is_empty() || arg.contains([' ', '\t', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// The Task Scheduler definition: run at sign-in of `user`, no window, no elevated rights, no
/// time limit, and no second copy while one runs.
pub fn windows_task_xml(exe: &Path, args: &[String], user: &str) -> String {
    let args: Vec<String> = args.iter().map(|a| quote_windows(a)).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>AIVory Code: start aivoryd at sign-in</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Hidden>true</Hidden>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>{args}</Arguments></Exec></Actions>
</Task>
"#,
        user = xml_escape(user),
        exe = xml_escape(&exe.to_string_lossy()),
        args = xml_escape(&args.join(" ")),
    )
}

pub fn launchd_plist(exe: &Path, args: &[String]) -> String {
    let mut list = format!("<string>{}</string>", xml_escape(&exe.to_string_lossy()));
    for a in args {
        list.push_str(&format!("<string>{}</string>", xml_escape(a)));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>{list}</array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#
    )
}

fn shell_line(exe: &Path, args: &[String]) -> String {
    let mut line = format!("'{}'", exe.display().to_string().replace('\'', "'\\''"));
    for a in args {
        line.push_str(&format!(" '{}'", a.replace('\'', "'\\''")));
    }
    line
}

pub fn systemd_unit(exe: &Path, args: &[String]) -> String {
    let mut exec = format!("\"{}\"", exe.display());
    for a in args {
        exec.push_str(&format!(" \"{a}\""));
    }
    format!(
        "[Unit]\nDescription=AIVory Code daemon\n\n[Service]\nType=simple\nExecStart={exec}\n\n[Install]\nWantedBy=default.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entries_use_the_serve_arguments_of_the_app() {
        let args = serve_args("aivory");
        assert_eq!(args, ["serve", "--iroh", "--endpoint", "aivory"]);
        let exe = Path::new("C:/Program Files/AIVory Code/aivoryd.exe");
        let xml = windows_task_xml(exe, &args, "PC\\me");
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<UserId>PC\\me</UserId>"));
        assert!(xml.contains("<Command>C:/Program Files/AIVory Code/aivoryd.exe</Command>"));
        assert!(xml.contains("<Arguments>serve --iroh --endpoint aivory</Arguments>"));
        let plist = launchd_plist(Path::new("/Applications/a/aivoryd"), &args);
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
        assert!(plist.contains("<string>--iroh</string>"));
        let unit = systemd_unit(Path::new("/opt/a b/aivoryd"), &args);
        assert!(unit.contains("ExecStart=\"/opt/a b/aivoryd\" \"serve\" \"--iroh\""));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn a_path_outside_a_package_stays_as_it_is() {
        let exe = Path::new("C:/Users/me/AppData/Local/Programs/AIVory Code/aivoryd.exe");
        assert_eq!(stable_exe(exe), exe);
    }

    #[test]
    fn the_fake_scheduler_writes_and_removes_the_entry() {
        let dir = std::env::temp_dir().join(format!("aivory-autostart-{}", std::process::id()));
        let os = OsScheduler::Fake(dir.clone());
        let exe = Path::new("/opt/aivoryd");
        apply_with(&os, true, exe, "test-ep").unwrap();
        let text = fs::read_to_string(dir.join("autostart.txt")).unwrap();
        assert_eq!(
            text,
            "'/opt/aivoryd' 'serve' '--iroh' '--endpoint' 'test-ep'"
        );
        apply_with(&os, false, exe, "test-ep").unwrap();
        assert!(!dir.join("autostart.txt").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
