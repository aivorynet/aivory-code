//! Scheduled tasks: the cron parser, the schedule file, and the entries in the scheduler of
//! the operating system.
//!
//! The daemon fires a due schedule itself while it runs (on each scan). For the time when it
//! does not run, each enabled schedule has one entry in the scheduler of the operating system
//! for its next fire time: Windows Task Scheduler (`schtasks /XML`), a systemd user timer (or
//! the crontab), or launchd. The entry runs `aivoryd fire <id> --due <ms>`, which starts the
//! daemon when needed and calls `schedules/fire`. After each fire the daemon writes the entry
//! for the next fire time. A fire with a `due` time that the daemon fired already does nothing.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aivory_protocol::UnixMs;
use aivory_protocol::schedules::{OsEntry, Schedule};
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike};

// ---- cron ----------------------------------------------------------------------------------

/// A parsed cron expression: five fields in local time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: Vec<bool>,
    hours: Vec<bool>,
    days: Vec<bool>,
    months: Vec<bool>,
    weekdays: Vec<bool>,
    any_day: bool,
    any_weekday: bool,
}

impl Cron {
    /// Parse `minute hour day-of-month month day-of-week`. Day of week: 0 or 7 is Sunday.
    pub fn parse(expr: &str) -> Result<Self, String> {
        let f: Vec<&str> = expr.split_whitespace().collect();
        if f.len() != 5 {
            return Err(format!(
                "a cron expression has 5 fields (minute hour day month weekday), not {}",
                f.len()
            ));
        }
        let mut weekdays = field(f[4], 0, 7, "weekday")?;
        if weekdays[7] {
            weekdays[0] = true;
        }
        weekdays.truncate(7);
        Ok(Self {
            minutes: field(f[0], 0, 59, "minute")?,
            hours: field(f[1], 0, 23, "hour")?,
            days: field(f[2], 1, 31, "day")?,
            months: field(f[3], 1, 12, "month")?,
            weekdays,
            any_day: f[2] == "*",
            any_weekday: f[4] == "*",
        })
    }

    fn day_matches(&self, t: &DateTime<Local>) -> bool {
        let dom = self.days[t.day() as usize];
        let dow = self.weekdays[t.weekday().num_days_from_sunday() as usize];
        // Classic cron: with both fields set, either one matches.
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (false, true) => dom,
            (true, false) => dow,
            (false, false) => dom || dow,
        }
    }

    /// The first matching minute after `after`. `None` when nothing matches in 5 years.
    pub fn next_after(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        let start = after.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let end = start + Duration::days(366 * 5);
        let mut t = start;
        while t < end {
            if !self.months[t.month() as usize] {
                t = local(
                    t.year() + i32::from(t.month() == 12),
                    t.month() % 12 + 1,
                    1,
                    0,
                    0,
                )?;
                continue;
            }
            if !self.day_matches(&t) {
                t = next_day(t)?;
                continue;
            }
            if !self.hours[t.hour() as usize] {
                t = t.with_minute(0)? + Duration::hours(1);
                continue;
            }
            if !self.minutes[t.minute() as usize] {
                t += Duration::minutes(1);
                continue;
            }
            return Some(t);
        }
        None
    }
}

fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> Option<DateTime<Local>> {
    Local.with_ymd_and_hms(y, m, d, h, min, 0).earliest()
}

fn next_day(t: DateTime<Local>) -> Option<DateTime<Local>> {
    let d = t.date_naive().succ_opt()?;
    local(d.year(), d.month(), d.day(), 0, 0)
}

/// One field: `*`, `n`, `a-b`, lists, and `/step`. The result has an entry for each value up
/// to `max` (index = value).
fn field(text: &str, min: u32, max: u32, name: &str) -> Result<Vec<bool>, String> {
    let mut out = vec![false; max as usize + 1];
    let bad = || format!("bad {name} field '{text}'");
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().map_err(|_| bad())?),
            None => (part, 1),
        };
        if step == 0 {
            return Err(bad());
        }
        let (a, b) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (
                a.parse::<u32>().map_err(|_| bad())?,
                b.parse::<u32>().map_err(|_| bad())?,
            )
        } else {
            let v = range.parse::<u32>().map_err(|_| bad())?;
            // `5/15` means from 5 to the end, every 15.
            (v, if part.contains('/') { max } else { v })
        };
        if a < min || b > max || a > b {
            return Err(bad());
        }
        let mut v = a;
        while v <= b {
            out[v as usize] = true;
            v += step;
        }
    }
    Ok(out)
}

/// The next fire time of a schedule after `now` (ms), or `None` when it is off or never
/// matches.
pub fn next_fire(s: &Schedule, now: UnixMs) -> Option<UnixMs> {
    if !s.enabled {
        return None;
    }
    let cron = Cron::parse(&s.cron).ok()?;
    let after = Local.timestamp_millis_opt(now).single()?;
    cron.next_after(after).map(|t| t.timestamp_millis())
}

// ---- the schedule file ---------------------------------------------------------------------

/// `<cache>/schedules.json`.
pub fn store_file() -> Option<PathBuf> {
    Some(aivory_sessions::cache_dir()?.join("schedules.json"))
}

pub fn load(path: &Path) -> Vec<Schedule> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, schedules: &[Schedule]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(schedules)?)?;
    fs::rename(tmp, path)
}

// ---- the scheduler of the operating system -------------------------------------------------

/// The scheduler that keeps the entries. `AIVORY_SCHEDULER=fake` writes the entries into a
/// folder (tests); `none` keeps no entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsScheduler {
    Windows,
    Systemd,
    Crontab,
    Launchd,
    /// Entries as files in this folder.
    Fake(PathBuf),
    None,
}

impl OsScheduler {
    pub fn detect() -> Self {
        match std::env::var("AIVORY_SCHEDULER").ok().as_deref() {
            Some("fake") => {
                let dir = aivory_sessions::cache_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("os-schedule");
                return Self::Fake(dir);
            }
            Some("none") => return Self::None,
            _ => {}
        }
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Launchd
        } else if run("systemctl", &["--user", "show-environment"], None).is_ok() {
            Self::Systemd
        } else {
            Self::Crontab
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Systemd => "systemd",
            Self::Crontab => "crontab",
            Self::Launchd => "launchd",
            Self::Fake(_) => "fake",
            Self::None => "none",
        }
    }

    /// Write the entry of schedule `id` for the fire time `at` (replaces an old one).
    pub fn install(&self, exe: &Path, id: &str, at: UnixMs) -> OsEntry {
        let mut entry = OsEntry {
            scheduler: self.label().to_string(),
            name: Some(entry_name(self, id)),
            at: Some(at),
            error: None,
        };
        let Some(t) = Local.timestamp_millis_opt(at).single() else {
            entry.error = Some("bad fire time".into());
            return entry;
        };
        if let Err(e) = self.write(exe, id, at, t) {
            entry.error = Some(e);
        }
        entry
    }

    fn write(&self, exe: &Path, id: &str, at: UnixMs, t: DateTime<Local>) -> Result<(), String> {
        match self {
            Self::Windows => {
                let file = std::env::temp_dir().join(format!("aivory-schedule-{id}.xml"));
                fs::write(&file, utf16_bom(&windows_task_xml(exe, id, at, t)))
                    .map_err(|e| e.to_string())?;
                let name = entry_name(self, id);
                let r = run(
                    "schtasks",
                    &[
                        "/Create",
                        "/TN",
                        &name,
                        "/XML",
                        &file.to_string_lossy(),
                        "/F",
                    ],
                    None,
                );
                let _ = fs::remove_file(&file);
                r
            }
            Self::Systemd => {
                let dir = systemd_dir().ok_or("no home folder")?;
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let (service, timer) = systemd_units(exe, id, at, t);
                let base = format!("aivory-schedule-{id}");
                fs::write(dir.join(format!("{base}.service")), service)
                    .map_err(|e| e.to_string())?;
                fs::write(dir.join(format!("{base}.timer")), timer).map_err(|e| e.to_string())?;
                run("systemctl", &["--user", "daemon-reload"], None)?;
                run(
                    "systemctl",
                    &["--user", "enable", "--now", &format!("{base}.timer")],
                    None,
                )
            }
            Self::Crontab => {
                let old = run_out("crontab", &["-l"]).unwrap_or_default();
                let new = crontab_merge(&old, id, Some(&crontab_line(exe, id, at, t)));
                run("crontab", &["-"], Some(&new))
            }
            Self::Launchd => {
                let path = launchd_file(id).ok_or("no home folder")?;
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                fs::write(&path, launchd_plist(exe, id, at, t)).map_err(|e| e.to_string())?;
                let uid = run_out("id", &["-u"])?.trim().to_string();
                let label = format!("gui/{uid}/net.aivory.schedule.{id}");
                let _ = run("launchctl", &["bootout", &label], None);
                run(
                    "launchctl",
                    &["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()],
                    None,
                )
            }
            Self::Fake(dir) => {
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                fs::write(
                    dir.join(format!("{id}.txt")),
                    format!("{at}\n{}", exe.display()),
                )
                .map_err(|e| e.to_string())
            }
            Self::None => Ok(()),
        }
    }

    /// Remove the entry of schedule `id`. A missing entry is no error.
    pub fn remove(&self, id: &str) {
        match self {
            Self::Windows => {
                let _ = run(
                    "schtasks",
                    &["/Delete", "/TN", &entry_name(self, id), "/F"],
                    None,
                );
            }
            Self::Systemd => {
                let base = format!("aivory-schedule-{id}");
                let _ = run(
                    "systemctl",
                    &["--user", "disable", "--now", &format!("{base}.timer")],
                    None,
                );
                if let Some(dir) = systemd_dir() {
                    let _ = fs::remove_file(dir.join(format!("{base}.service")));
                    let _ = fs::remove_file(dir.join(format!("{base}.timer")));
                }
                let _ = run("systemctl", &["--user", "daemon-reload"], None);
            }
            Self::Crontab => {
                if let Ok(old) = run_out("crontab", &["-l"]) {
                    let _ = run("crontab", &["-"], Some(&crontab_merge(&old, id, None)));
                }
            }
            Self::Launchd => {
                if let Ok(uid) = run_out("id", &["-u"]) {
                    let label = format!("gui/{}/net.aivory.schedule.{id}", uid.trim());
                    let _ = run("launchctl", &["bootout", &label], None);
                }
                if let Some(p) = launchd_file(id) {
                    let _ = fs::remove_file(p);
                }
            }
            Self::Fake(dir) => {
                let _ = fs::remove_file(dir.join(format!("{id}.txt")));
            }
            Self::None => {}
        }
    }
}

fn entry_name(s: &OsScheduler, id: &str) -> String {
    match s {
        OsScheduler::Windows => format!("AIVory\\{id}"),
        OsScheduler::Launchd => format!("net.aivory.schedule.{id}"),
        _ => format!("aivory-schedule-{id}"),
    }
}

fn run(program: &str, args: &[&str], stdin: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn run_out(program: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    no_window(&mut cmd);
    let out = cmd.output().map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{program} failed"))
    }
}

/// No console window for the scheduler tools on Windows.
fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The Task Scheduler definition: one time trigger in local time, run when the PC was off at
/// that time (`StartWhenAvailable`), no window, no elevated rights.
pub fn windows_task_xml(exe: &Path, id: &str, at: UnixMs, t: DateTime<Local>) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>AIVory Code scheduled task {id}</Description></RegistrationInfo>
  <Triggers><TimeTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled></TimeTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT10M</ExecutionTimeLimit>
    <Hidden>true</Hidden>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>fire {id} --due {at}</Arguments></Exec></Actions>
</Task>
"#,
        start = t.format("%Y-%m-%dT%H:%M:%S"),
        exe = xml_escape(&exe.to_string_lossy()),
    )
}

fn utf16_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn systemd_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))?;
    Some(config.join("systemd").join("user"))
}

/// The service and the timer of a systemd user timer. `Persistent` runs a missed fire after
/// the next boot.
pub fn systemd_units(exe: &Path, id: &str, at: UnixMs, t: DateTime<Local>) -> (String, String) {
    let service = format!(
        "[Unit]\nDescription=AIVory Code scheduled task {id}\n\n[Service]\nType=oneshot\nExecStart=\"{}\" fire {id} --due {at}\n",
        exe.display()
    );
    let timer = format!(
        "[Unit]\nDescription=AIVory Code scheduled task {id}\n\n[Timer]\nOnCalendar={}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n",
        t.format("%Y-%m-%d %H:%M:%S")
    );
    (service, timer)
}

const CRON_MARK: &str = "# aivory-schedule ";

/// The crontab line of one fire time (minute, hour, day, month; any weekday).
pub fn crontab_line(exe: &Path, id: &str, at: UnixMs, t: DateTime<Local>) -> String {
    format!(
        "{} {} {} {} * '{}' fire {id} --due {at}",
        t.minute(),
        t.hour(),
        t.day(),
        t.month(),
        exe.display()
    )
}

/// The crontab with the lines of schedule `id` replaced by `line` (or removed). The other
/// lines of the user stay as they are.
pub fn crontab_merge(old: &str, id: &str, line: Option<&str>) -> String {
    let mark = format!("{CRON_MARK}{id}");
    let mut out = String::new();
    let mut skip = false;
    for l in old.lines() {
        if skip {
            skip = false;
            continue;
        }
        if l.trim() == mark {
            skip = true;
            continue;
        }
        let _ = writeln!(out, "{l}");
    }
    if let Some(line) = line {
        let _ = writeln!(out, "{mark}\n{line}");
    }
    out
}

fn launchd_file(id: &str) -> Option<PathBuf> {
    Some(
        std::env::home_dir()?
            .join("Library")
            .join("LaunchAgents")
            .join(format!("net.aivory.schedule.{id}.plist")),
    )
}

pub fn launchd_plist(exe: &Path, id: &str, at: UnixMs, t: DateTime<Local>) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>net.aivory.schedule.{id}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>fire</string><string>{id}</string><string>--due</string><string>{at}</string></array>
  <key>StartCalendarInterval</key>
  <dict><key>Minute</key><integer>{min}</integer><key>Hour</key><integer>{hour}</integer><key>Day</key><integer>{day}</integer><key>Month</key><integer>{month}</integer></dict>
</dict>
</plist>
"#,
        exe = xml_escape(&exe.to_string_lossy()),
        min = t.minute(),
        hour = t.hour(),
        day = t.day(),
        month = t.month(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        local(y, m, d, h, min).unwrap()
    }

    #[test]
    fn cron_next_fire_times() {
        let every_15 = Cron::parse("*/15 * * * *").unwrap();
        assert_eq!(
            every_15.next_after(at(2026, 11, 9, 10, 7)),
            Some(at(2026, 11, 9, 10, 15))
        );
        let weekday_9 = Cron::parse("0 9 * * 1-5").unwrap();
        // 2026-11-14 is a Saturday: the next fire is Monday 9:00.
        assert_eq!(
            weekday_9.next_after(at(2026, 11, 14, 8, 0)),
            Some(at(2026, 11, 16, 9, 0))
        );
        let first = Cron::parse("30 2 1 * *").unwrap();
        assert_eq!(
            first.next_after(at(2026, 12, 15, 0, 0)),
            Some(at(2027, 1, 1, 2, 30))
        );
        let sunday = Cron::parse("0 0 * * 7").unwrap();
        assert_eq!(
            sunday.next_after(at(2026, 11, 9, 0, 0)),
            Some(at(2026, 11, 15, 0, 0))
        );
        assert_eq!(
            Cron::parse("0 0 31 2 *")
                .unwrap()
                .next_after(at(2026, 1, 1, 0, 0)),
            None
        );
    }

    #[test]
    fn cron_rejects_bad_fields() {
        assert!(Cron::parse("* * * *").is_err());
        assert!(Cron::parse("60 * * * *").is_err());
        assert!(Cron::parse("*/0 * * * *").is_err());
        assert!(Cron::parse("5-1 * * * *").is_err());
        assert!(Cron::parse("x * * * *").is_err());
        assert!(Cron::parse("0,30 8-18/2 * 1,6 *").is_ok());
    }

    #[test]
    fn os_entries_name_the_fire_command() {
        let t = at(2026, 11, 9, 14, 30);
        let exe = Path::new("C:/Program Files/AIVory/aivoryd.exe");
        let xml = windows_task_xml(exe, "s1", 42, t);
        assert!(xml.contains("<StartBoundary>2026-11-09T14:30:00</StartBoundary>"));
        assert!(xml.contains("<Arguments>fire s1 --due 42</Arguments>"));
        assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
        assert_eq!(&utf16_bom("a")[..], &[0xFF, 0xFE, b'a', 0]);
        let (service, timer) = systemd_units(exe, "s1", 42, t);
        assert!(service.contains("fire s1 --due 42"));
        assert!(timer.contains("OnCalendar=2026-11-09 14:30:00"));
        assert!(timer.contains("Persistent=true"));
        assert!(crontab_line(exe, "s1", 42, t).starts_with("30 14 9 11 * '"));
        let plist = launchd_plist(exe, "s1", 42, t);
        assert!(plist.contains("<key>Hour</key><integer>14</integer>"));
    }

    #[test]
    fn crontab_merge_keeps_the_lines_of_the_user() {
        let old = "MAILTO=x\n0 1 * * * backup\n# aivory-schedule s1\n1 2 3 4 * old\n";
        let new = crontab_merge(old, "s1", Some("5 6 7 8 * new"));
        assert_eq!(
            new,
            "MAILTO=x\n0 1 * * * backup\n# aivory-schedule s1\n5 6 7 8 * new\n"
        );
        assert_eq!(
            crontab_merge(&new, "s1", None),
            "MAILTO=x\n0 1 * * * backup\n"
        );
    }

    #[test]
    fn the_fake_scheduler_writes_and_removes_entries() {
        let dir = std::env::temp_dir().join(format!("aivory-osched-{}", std::process::id()));
        let s = OsScheduler::Fake(dir.clone());
        let e = s.install(Path::new("aivoryd"), "s1", 1_800_000_000_000);
        assert_eq!(e.error, None);
        assert_eq!(e.scheduler, "fake");
        assert!(dir.join("s1.txt").is_file());
        s.remove("s1");
        assert!(!dir.join("s1.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
