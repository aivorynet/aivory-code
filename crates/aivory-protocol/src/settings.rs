//! Settings (`Settings.dc.html`), daemons, devices, account (phone Me).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actions::{AcpAgent, PinnedRepo};
use crate::common::{Effort, Mode, UnixMs};
use crate::warroom::{Autopilot, LinkState, UsageWindow};

/// Defaults of one provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPrefs {
    pub enabled: bool,
    pub model: String,
    pub effort: Effort,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    Light,
    Dark,
    System,
}

/// Console background of the session panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ConsoleBackground {
    /// #FFFFFF
    White,
    /// #F4F5F7
    Grey,
    /// #15161B, text #E4E5EA
    Dark,
    /// #0D1624, text #E4E7F0
    Navy,
}

/// The look of the whole desktop app (ADR 0022). Each look draws every screen from the same
/// data. `Display.theme` (light, dark) applies to Minimal only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Look {
    /// The orchestration board: modules, streams, gates, log, queue, live.
    #[default]
    Minimal,
    /// A starship console: big readouts, a schematic, numbered rows.
    #[serde(alias = "lcars")]
    Bridge,
    /// An operator wall of numbered monitors, one code rain per agent.
    #[serde(alias = "matrix")]
    Operator,
    /// A vector city map: one district per codebase, agents move between parts.
    #[serde(alias = "nightCity", alias = "cyber")]
    Neon,
    /// A 3D file-system landscape in the style of early-90s workstation software.
    Navigator,
    /// See-through glass panels over slow rings; a bloom of strands, one per session.
    Holo,
    /// A phosphor-green radar: bearing is the codebase, range is time since the last move.
    Tactical,
    /// A real-time strategy command console: a map of units per codebase, a command sidebar.
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Display {
    pub theme: Theme,
    /// Panes per row: 1, 2, 3, 4, 8, 9 or 16. `None` is Auto (fits the window width).
    pub per_row: Option<u8>,
    /// Console font family.
    pub font: String,
    /// Console font size, 9-18 px.
    pub font_size: u8,
    pub background: ConsoleBackground,
    /// Interface text size in percent of the base sizes, 80-150.
    #[serde(default = "Display::default_ui_scale")]
    pub ui_scale: u16,
    #[serde(default, alias = "skin")]
    pub look: Look,
    /// The id of an installed theme package (ADR 0024). `None` uses `look`. Setting
    /// `display.look` clears it. The field serializes as `null`, so `set_key` finds it.
    #[serde(default)]
    pub custom_theme: Option<String>,
    /// The maximize button of the window opens a borderless full screen. F11 goes back.
    #[serde(default)]
    pub maximize_full_screen: bool,
}

impl Display {
    pub const FONTS: [&'static str; 5] = [
        "JetBrains Mono",
        "IBM Plex Mono",
        "Fira Code",
        "Source Code Pro",
        "Roboto Mono",
    ];
    pub const FONT_SIZE: (u8, u8) = (9, 18);
    pub const UI_SCALE: (u16, u16) = (80, 150);

    /// The owner reads the interface best a bit larger than the base sizes (2026-10-23).
    pub fn default_ui_scale() -> u16 {
        115
    }
}

impl Default for Display {
    fn default() -> Self {
        Self {
            theme: Theme::Light,
            per_row: None,
            font: Self::FONTS[0].to_string(),
            // 12 px was too small (2026-10-23); 16 px panes were too loud in war room v2.
            font_size: 14,
            // War room v2 (2026-10-27): light panes on the white page; dark is a Display option.
            background: ConsoleBackground::White,
            ui_scale: Self::default_ui_scale(),
            look: Look::Minimal,
            custom_theme: None,
            maximize_full_screen: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Channels {
    pub desktop: bool,
    pub phone: bool,
    pub email: bool,
}

/// Notification events × channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Notify {
    /// Question, permission or plan.
    pub needs_answer: Channels,
    /// A session stops on an error.
    pub error: Channels,
    /// A task reaches the review gate.
    pub review: Channels,
    /// Two sessions edit the same code.
    pub conflict: Channels,
    /// A session parks or a queued task starts.
    pub autopilot: Channels,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuietHours {
    pub on: bool,
    /// `HH:MM`, local time.
    pub from: String,
    /// `HH:MM`, local time.
    pub to: String,
}

/// Settings of the remote link and the phone push.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    /// The URL of the push gateway (`https://.../v1/push`). Empty: no push to the phones.
    #[serde(default)]
    pub push_url: String,
    /// The relay of the iroh link: a URL, `default` (the iroh relays), or `none`. Empty means
    /// `default`. `aivoryd serve --relay` wins over this value.
    #[serde(default)]
    pub relay: String,
}

/// Settings of the turn ledger (`docs/design/05-vcs-ledger.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerPrefs {
    /// Take a snapshot of the repository at each turn end of a session (hidden commits on
    /// `refs/aivory/turns/...`; the working tree and the index stay as they are).
    #[serde(default = "yes")]
    pub snapshots: bool,
}

fn yes() -> bool {
    true
}

impl Default for LedgerPrefs {
    fn default() -> Self {
        Self { snapshots: true }
    }
}

/// Everything that `settings/set` can change. Changes save at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Key: provider id.
    pub providers: BTreeMap<String, ProviderPrefs>,
    pub autopilot: Autopilot,
    pub display: Display,
    pub notify: Notify,
    pub quiet_hours: QuietHours,
    #[serde(default)]
    pub remote: Remote,
    #[serde(default)]
    pub ledger: LedgerPrefs,
    /// The ACP agents of the user (`providers/addAcp`). Each one shows as a provider card.
    #[serde(default)]
    pub acp: Vec<AcpAgent>,
    /// The codebases of the user (`repos/add`): the daemon scans them without a session.
    #[serde(default)]
    pub pinned: Vec<PinnedRepo>,
    /// One quiet coffee hint per app start. Old JSON has no value: on.
    #[serde(default = "Settings::default_coffee_reminder")]
    pub coffee_reminder: bool,
}

impl Settings {
    pub fn default_coffee_reminder() -> bool {
        true
    }

    /// Set one value by its dotted camelCase key, for example `autopilot.parkAfterMin`,
    /// `providers.claude.effort`, `display.perRow` or `notify.error.phone`. The key must
    /// exist and the value must fit its type. Numbers come back clamped to their range.
    pub fn set_key(&mut self, key: &str, value: Value) -> Result<(), String> {
        // These lists have their own methods, which check the input.
        let root = key.split('.').next().unwrap_or_default();
        if matches!(root, "acp" | "pinned") {
            return Err(format!(
                "'{root}' has its own methods: providers/addAcp, repos/add"
            ));
        }
        let mut doc = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        let mut node = &mut doc;
        for part in key.split('.') {
            node = node
                .as_object_mut()
                .and_then(|m| m.get_mut(part))
                .ok_or_else(|| format!("unknown settings key '{key}'"))?;
        }
        *node = value;
        let mut next: Settings =
            serde_json::from_value(doc).map_err(|e| format!("invalid value for '{key}': {e}"))?;
        next.autopilot.clamp();
        if key == "display.look" {
            next.display.custom_theme = None;
        }
        let (lo, hi) = Display::FONT_SIZE;
        next.display.font_size = next.display.font_size.clamp(lo, hi);
        let (lo, hi) = Display::UI_SCALE;
        next.display.ui_scale = next.display.ui_scale.clamp(lo, hi);
        *self = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProviderStatus {
    Connected,
    /// An ACP agent.
    Acp,
    Off,
}

/// A provider card of Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    /// One letter for the card monogram.
    pub monogram: String,
    /// The auth line, for example `subscription · Max · signed in`.
    pub auth: String,
    pub status: ProviderStatus,
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage: Vec<UsageWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DaemonKind {
    /// This machine.
    Local,
    Ssh,
    Cloud,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Daemon {
    pub id: String,
    pub name: String,
    pub kind: DaemonKind,
    /// `aivoryd 0.9.2 · Windows 11`, `ssh pp@build-01`, `cloud runner · eu-central`.
    pub detail: String,
    pub sessions: u32,
    pub running: bool,
}

/// A paired phone or desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub os: String,
    pub paired: UnixMs,
    pub last_seen: UnixMs,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub team: String,
    pub seats: u32,
    pub license: String,
    /// `YYYY-MM-DD`.
    pub renews: String,
    pub hub: String,
    pub relay: String,
    pub hub_state: LinkState,
}

/// The monthly budget (phone Me).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    /// `YYYY-MM`.
    pub month: String,
    /// USD.
    pub spent: f64,
    /// USD.
    pub limit: f64,
    pub warn_pct: u8,
}

/// Result of `settings/get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub settings: Settings,
    pub providers: Vec<ProviderInfo>,
    pub daemons: Vec<Daemon>,
    pub devices: Vec<Device>,
    pub account: Account,
    pub budget: Budget,
}

/// The push service of a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PushPlatform {
    Apns,
    Fcm,
}

impl PushPlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Fcm => "fcm",
        }
    }
}

/// Parameters of `devices/push`: where the daemon sends the end-to-end encrypted push of a
/// paired device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DevicePushParams {
    /// The device id (`device_id` of the pairing). The daemon binds the call to the caller, so a
    /// client may leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The APNs hex token or the FCM registration token.
    pub token: String,
    pub platform: PushPlatform,
    /// The X25519 public key of the device for sealed pushes, base64url without padding.
    pub push_key: String,
}

/// Result of `devices/pair`: show `url` as a QR code, or `code` for typing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub code: String,
    pub url: String,
    pub expires: UnixMs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_skin_key_reads_as_look() {
        let mut v = serde_json::to_value(Display::default()).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("look");
        o.insert("skin".into(), "cyber".into());
        let d: Display = serde_json::from_value(v).unwrap();
        assert_eq!(d.look, Look::Neon);
    }

    #[test]
    fn old_settings_without_custom_theme_load() {
        let mut v = serde_json::to_value(crate::demo::Demo::new().settings.settings).unwrap();
        v["display"].as_object_mut().unwrap().remove("customTheme");
        let s: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(s.display.custom_theme, None);
    }

    #[test]
    fn custom_theme_is_a_settable_key_and_a_look_change_clears_it() {
        let mut s = crate::demo::Demo::new().settings.settings;
        s.set_key("display.customTheme", "my-theme".into()).unwrap();
        assert_eq!(s.display.custom_theme.as_deref(), Some("my-theme"));
        s.set_key("display.look", "holo".into()).unwrap();
        assert_eq!(s.display.custom_theme, None);
        assert_eq!(s.display.look, Look::Holo);
    }
}
