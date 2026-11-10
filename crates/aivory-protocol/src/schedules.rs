//! Scheduled tasks: a cron expression and a task template. At each fire time the daemon
//! creates the task and starts its first session. The daemon also keeps one entry in the
//! scheduler of the operating system (Windows Task Scheduler, a systemd user timer or the
//! crontab, launchd), so that a fire time starts the daemon when it does not run.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{ComposerPrefs, UnixMs};
use crate::tasks::Playbook;

/// What a fire creates: a task with its first session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskTemplate {
    pub title: String,
    pub codebase: String,
    /// Default: `general`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default = "default_playbook")]
    pub playbook: Playbook,
    pub prompt: String,
    /// Default: the provider defaults of Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<ComposerPrefs>,
}

fn default_playbook() -> Playbook {
    Playbook::Feature
}

/// The state of the entry in the scheduler of the operating system.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OsEntry {
    /// `windows`, `systemd`, `crontab`, `launchd`, or `none` (the daemon fires it while it
    /// runs).
    pub scheduler: String,
    /// The name of the entry, for example `AIVory\s1731150000000`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The fire time that the entry holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<UnixMs>,
    /// Why the entry could not be written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: String,
    pub name: String,
    /// Five fields in local time: minute, hour, day of month, month, day of week (0 = Sunday).
    /// Each field: `*`, a number, a range `1-5`, a list `1,15`, and a step `*/15`.
    pub cron: String,
    pub enabled: bool,
    pub template: TaskTemplate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired: Option<UnixMs>,
    /// The task of the last fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_task: Option<String>,
    /// `None` when the schedule is off or the expression never matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fire: Option<UnixMs>,
    #[serde(default)]
    pub os_entry: OsEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleList {
    pub schedules: Vec<Schedule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleCreateParams {
    pub name: String,
    pub cron: String,
    pub template: TaskTemplate,
    /// Default: `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleUpdateParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<TaskTemplate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRef {
    pub id: String,
}

/// `schedules/fire`: the entry of the operating system calls it. `due` is the fire time that
/// the entry was for: a fire that the daemon did already is not repeated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleFireParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<UnixMs>,
}
