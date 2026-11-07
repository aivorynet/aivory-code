//! Theme packages (ADR 0024): the manifest, the tokens, the layout file, the registry, and the
//! types of the `theme/*` and `registry/*` methods.
//!
//! Files of a package use `snake_case` keys (TOML and JSON by hand). The RPC types use
//! camelCase like the rest of the protocol.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The component catalog version that this build draws (ADR 0024 section 4).
pub const CATALOG_VERSION: u32 = 1;

/// The id of the official registry.
pub const OFFICIAL_REGISTRY: &str = "@aivory";
/// The URL of the official registry.
pub const OFFICIAL_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/aivorynet/aivory-themes/main/registry.json";

/// A display class of a layout set.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DisplayClass {
    Desktop,
    Phone,
    Fold,
    Watch,
}

impl DisplayClass {
    pub const ALL: [Self; 4] = [Self::Desktop, Self::Phone, Self::Fold, Self::Watch];

    /// The folder name below `layouts/`.
    pub fn dir(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Phone => "phone",
            Self::Fold => "fold",
            Self::Watch => "watch",
        }
    }
}

/// The author of a theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn default_token_files() -> Vec<String> {
    vec!["tokens.toml".to_string()]
}

/// `theme.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeManifest {
    /// Lowercase `[a-z0-9-]`, 3 to 40 characters.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// SemVer, equal to the Git tag without the `v`.
    pub version: String,
    pub author: Author,
    /// An SPDX expression for the theme files.
    pub license: String,
    /// The lowest app version (SemVer).
    pub min_app: String,
    /// The component catalog version that the theme needs.
    pub catalog: u32,
    /// `desktop` is required.
    pub displays: Vec<DisplayClass>,
    /// The token files in order; a later file overrides keys of an earlier file.
    #[serde(default = "default_token_files")]
    pub tokens: Vec<String>,
}

/// One color for each session state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateColors {
    pub needs_you: Option<String>,
    pub working: Option<String>,
    pub idle: Option<String>,
    pub parked: Option<String>,
    pub error: Option<String>,
    pub done: Option<String>,
}

/// Colors as `#rrggbb` or `#rrggbbaa`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    pub bg: Option<String>,
    pub surface: Option<String>,
    pub text: Option<String>,
    pub muted: Option<String>,
    pub accent: Option<String>,
    pub analysis: Option<String>,
    pub state: Option<StateColors>,
}

/// A family is a bundled font file (`assets/fonts/...`) or `sans`, `serif`, `mono`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fonts {
    pub ui: Option<String>,
    pub mono: Option<String>,
}

/// The five text sizes in px.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeSizes {
    pub xs: Option<f64>,
    pub sm: Option<f64>,
    pub md: Option<f64>,
    pub lg: Option<f64>,
    pub xl: Option<f64>,
}

/// Durations in ms and an easing name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Motion {
    #[serde(default)]
    pub durations: BTreeMap<String, u32>,
    pub easing: Option<String>,
}

/// Effect strengths, each with a fixed range (see the validator).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Effects {
    /// 0 to 1.
    pub glow: Option<f64>,
    /// 0 to 1.
    pub scanlines: Option<f64>,
    /// 0 to 1.
    pub glass_alpha: Option<f64>,
    /// 0 to 40 px.
    pub blur: Option<f64>,
}

/// `tokens.toml`. The table `type` holds the text sizes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenFile {
    #[serde(default)]
    pub colors: Colors,
    #[serde(default)]
    pub fonts: Fonts,
    #[serde(default, rename = "type")]
    pub type_sizes: TypeSizes,
    #[serde(default)]
    pub space: BTreeMap<String, f64>,
    #[serde(default)]
    pub radii: BTreeMap<String, f64>,
    #[serde(default)]
    pub motion: Motion,
    #[serde(default)]
    pub effects: Effects,
}

/// A layout file: a component tree below `root`. The tree has a fixed grammar that the
/// validator checks (ADR 0024 section 3), so the schema keeps `root` open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutFile {
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub root: Value,
}

/// The type of a registry item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    Theme,
    Screen,
    Parts,
    Tokens,
}

/// Where the files of an item are: one release asset URL, or a list of raw file URLs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ItemSource {
    Archive(String),
    Files(Vec<String>),
}

/// One item of a registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryItem {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ItemType,
    pub version: String,
    pub title: String,
    pub author: String,
    pub license: String,
    #[serde(default)]
    pub description: String,
    /// An image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    pub source: ItemSource,
    /// SHA-256 of the archive, in lowercase hex.
    pub sha256: String,
    pub min_app: String,
    pub catalog: u32,
    pub displays: Vec<DisplayClass>,
    /// Other items.
    #[serde(default)]
    pub requires: Vec<String>,
}

/// `registry.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub items: Vec<RegistryItem>,
}

// ---- RPC ----

/// `theme/inspect`: a source such as `github:owner/repo@v1.0.0` or a GitHub release URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSourceParams {
    pub source: String,
}

/// A started job. Poll it with `theme/job`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobRef {
    pub job: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Pending,
    Done,
    Failed,
}

/// The state of a job. `result` has the type of the method that started the job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub job: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The result of `theme/inspect`: what the add dialog shows. `token` goes to `theme/add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Inspected {
    pub token: String,
    pub id: String,
    pub name: String,
    pub author: String,
    pub license: String,
    pub version: String,
    pub source: String,
    /// Bytes of the archive.
    pub archive_bytes: u64,
    /// Bytes of all files after unpacking.
    pub size: u64,
    pub files: u32,
    /// SHA-256 of the archive.
    pub sha256: String,
    /// The theme is installed already (an add fails; use `theme/update`).
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeAddParams {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeRef {
    pub id: String,
}

/// An installed theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstalledTheme {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub license: String,
    pub source: String,
    /// Files that the user changed since the add.
    pub edited: Vec<String>,
    /// The theme is invalid; the app uses Minimal and shows this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeList {
    pub themes: Vec<InstalledTheme>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoveResult {
    pub ok: bool,
    /// The theme was the current theme; `display.customTheme` is clear now.
    pub cleared_current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAvailable {
    pub id: String,
    pub current: String,
    pub latest: String,
}

/// The result of `theme/checkUpdates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateList {
    pub updates: Vec<UpdateAvailable>,
    /// Themes whose check failed: `id: message`.
    pub errors: Vec<String>,
}

/// The result of `theme/diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeDiff {
    pub id: String,
    pub current_version: String,
    pub latest_version: String,
    pub local_edits: Vec<String>,
    pub upstream_changed: Vec<String>,
    pub upstream_added: Vec<String>,
    pub upstream_removed: Vec<String>,
}

/// The result of `theme/update`. A file that the user edited stays, and the new upstream file
/// is next to it as `<file>.upstream`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    pub id: String,
    pub version: String,
    pub replaced: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Edited files with a `.upstream` copy.
    pub kept: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryAddParams {
    /// `@name`.
    pub name: String,
    /// An HTTPS URL of a `registry.json`.
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryRef {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryRefreshParams {
    /// One registry, or all when absent.
    #[serde(default)]
    pub name: Option<String>,
}

/// A registry of the user, with the items of the last refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub name: String,
    pub url: String,
    pub official: bool,
    /// Unix ms of the last refresh. `None`: never refreshed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<i64>,
    pub items: Vec<RegistryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryList {
    pub registries: Vec<RegistryEntry>,
}

/// `theme/get`: one installed theme for one display class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeGetParams {
    pub id: String,
    pub display: DisplayClass,
}

/// A font file of a theme and the family name inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeFont {
    /// The path below the theme folder, for `theme/asset`.
    pub file: String,
    pub family: String,
}

/// The result of `theme/get`: a resolved bundle for one display class. Read-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeBundle {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub license: String,
    /// SHA-256 of the archive from `theme.lock`; a theme without a lock has the hash of its
    /// manifest and tokens.
    pub sha256: String,
    pub display: DisplayClass,
    /// The merged tokens (the shape of `TokenFile`).
    pub tokens: Value,
    /// The `root` node of each screen of the class, by screen name.
    pub layouts: BTreeMap<String, Value>,
    /// The `root` node of each part, by name.
    pub parts: BTreeMap<String, Value>,
    /// `strings[lang][dotted key]`.
    pub strings: BTreeMap<String, BTreeMap<String, String>>,
    pub fonts: Vec<ThemeFont>,
    /// The path of `previews/<class>.png` when the theme has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// `theme/asset`: one file of `assets/` or `previews/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeAssetParams {
    pub id: String,
    pub path: String,
}

/// The result of `theme/asset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeAsset {
    pub path: String,
    pub bytes: u64,
    pub base64: String,
}

/// The schema of `theme.toml`.
pub fn manifest_schema() -> Value {
    schema_of::<ThemeManifest>("AIVory theme manifest (theme.toml)")
}

/// The schema of `tokens.toml`.
pub fn tokens_schema() -> Value {
    schema_of::<TokenFile>("AIVory theme tokens (tokens.toml)")
}

/// The schema of `registry.json`.
pub fn registry_schema() -> Value {
    schema_of::<Registry>("AIVory registry (registry.json)")
}

/// The schema of a layout file.
pub fn layout_schema() -> Value {
    schema_of::<LayoutFile>("AIVory theme layout (layouts/*/*.json)")
}

fn schema_of<T: JsonSchema>(title: &str) -> Value {
    let mut v = serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("title".to_string(), title.into());
    }
    v
}
