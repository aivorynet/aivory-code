//! A small serde subset of SARIF 2.1.0 and the stable fingerprint of a finding.
//!
//! Only the parts that the pipeline needs are modeled: runs, the tool, results with a rule id,
//! a level, a message, one or more locations (file and region), and `partialFingerprints`.
//! Unknown fields in a SARIF file are ignored when it is read.
//!
//! Lines and columns in a [`Region`] start at 1 (the SARIF rule). A column counts characters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SARIF_VERSION: &str = "2.1.0";
pub const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
/// The key of the stable fingerprint in `partialFingerprints`.
pub const FINGERPRINT_KEY: &str = "aivoryFingerprint/v1";

/// A SARIF log: a list of runs, one run per tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifLog {
    pub version: String,
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default)]
    pub runs: Vec<Run>,
}

impl Default for SarifLog {
    fn default() -> Self {
        Self {
            version: SARIF_VERSION.to_owned(),
            schema: Some(SARIF_SCHEMA.to_owned()),
            runs: Vec::new(),
        }
    }
}

impl SarifLog {
    /// All results of all runs.
    pub fn results(&self) -> impl Iterator<Item = &SarifResult> {
        self.runs.iter().flat_map(|r| r.results.iter())
    }

    /// The number of results of all runs.
    pub fn result_count(&self) -> usize {
        self.runs.iter().map(|r| r.results.len()).sum()
    }
}

/// One run of one tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub tool: Tool,
    #[serde(default)]
    pub results: Vec<SarifResult>,
}

impl Run {
    pub fn new(tool_name: &str) -> Self {
        Self {
            tool: Tool {
                driver: ToolComponent {
                    name: tool_name.to_owned(),
                    version: None,
                    rules: Vec::new(),
                },
            },
            results: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub driver: ToolComponent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolComponent {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ReportingDescriptor>,
}

/// The description of a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportingDescriptor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_description: Option<Message>,
}

/// The severity of a result.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    None,
    Note,
    #[default]
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactLocation {
    /// A path relative to the repo root, with `/` as separator.
    pub uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub start_line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
}

impl Region {
    /// A region from a start line and a start column (both start at 1).
    pub fn at(line: u32, column: u32) -> Self {
        Self {
            start_line: line,
            start_column: Some(column),
            end_line: None,
            end_column: None,
        }
    }

    /// A region with both ends (lines and columns start at 1).
    pub fn span(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Self {
        Self {
            start_line,
            start_column: Some(start_col),
            end_line: Some(end_line),
            end_column: Some(end_col),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalLocation {
    pub artifact_location: ArtifactLocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub physical_location: PhysicalLocation,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifResult {
    pub rule_id: String,
    #[serde(default)]
    pub level: Level,
    pub message: Message,
    #[serde(default)]
    pub locations: Vec<Location>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub partial_fingerprints: BTreeMap<String, String>,
}

impl SarifResult {
    /// The file of the first location.
    pub fn file(&self) -> Option<&str> {
        self.locations
            .first()
            .map(|l| l.physical_location.artifact_location.uri.as_str())
    }

    /// The region of the first location.
    pub fn region(&self) -> Option<Region> {
        self.locations
            .first()
            .and_then(|l| l.physical_location.region)
    }

    /// The first line of the first location (starts at 1).
    pub fn start_line(&self) -> Option<u32> {
        self.region().map(|r| r.start_line)
    }

    /// The stable fingerprint, when the result has one.
    pub fn fingerprint(&self) -> Option<&str> {
        self.partial_fingerprints
            .get(FINGERPRINT_KEY)
            .map(String::as_str)
    }

    /// A result with one location and no fingerprint yet.
    pub fn new(rule_id: &str, level: Level, message: &str, file: &str, region: Region) -> Self {
        Self {
            rule_id: rule_id.to_owned(),
            level,
            message: Message {
                text: message.to_owned(),
            },
            locations: vec![Location {
                physical_location: PhysicalLocation {
                    artifact_location: ArtifactLocation {
                        uri: file.to_owned(),
                    },
                    region: Some(region),
                },
            }],
            partial_fingerprints: BTreeMap::new(),
        }
    }

    /// Set the fingerprint from the flagged line and the enclosing declaration.
    pub fn with_fingerprint(mut self, line_text: &str, declaration: Option<&str>) -> Self {
        let file = self.file().unwrap_or("").to_owned();
        let fp = fingerprint(&self.rule_id, &file, line_text, declaration);
        self.partial_fingerprints
            .insert(FINGERPRINT_KEY.to_owned(), fp);
        self
    }
}

/// Collapse every run of white space to one space and trim the ends.
pub fn normalize_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The stable fingerprint of a finding: a hash of the rule id, the file, the normalized text of
/// the flagged line, and the name of the enclosing declaration. The line number is not part of
/// it, so a block that moves inside its declaration keeps its fingerprint.
pub fn fingerprint(
    rule_id: &str,
    file: &str,
    line_text: &str,
    declaration: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [
        rule_id,
        file,
        &normalize_line(line_text),
        declaration.unwrap_or(""),
    ] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().as_str()[..32].to_owned()
}

/// Make the fingerprints of a list unique: the second result with the same base fingerprint gets
/// the suffix `#1`, the third `#2`, and so on, in the order of the list. Results without a
/// fingerprint stay as they are.
pub fn disambiguate(results: &mut [SarifResult]) {
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for r in results {
        let Some(fp) = r.partial_fingerprints.get_mut(FINGERPRINT_KEY) else {
            continue;
        };
        let n = seen.entry(fp.clone()).or_insert(0);
        if *n > 0 {
            fp.push_str(&format!("#{n}"));
        }
        *n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_line_number_and_spacing() {
        let a = SarifResult::new("r1", Level::Warning, "m", "src/a.rs", Region::at(3, 1))
            .with_fingerprint("    let x = y.unwrap();", Some("run"));
        let b = SarifResult::new("r1", Level::Warning, "m", "src/a.rs", Region::at(40, 9))
            .with_fingerprint("let   x = y.unwrap();  ", Some("run"));
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint().unwrap().len(), 32);
    }

    #[test]
    fn fingerprint_changes_with_each_part() {
        let base = fingerprint("r", "f", "x", Some("d"));
        assert_ne!(base, fingerprint("r2", "f", "x", Some("d")));
        assert_ne!(base, fingerprint("r", "f2", "x", Some("d")));
        assert_ne!(base, fingerprint("r", "f", "y", Some("d")));
        assert_ne!(base, fingerprint("r", "f", "x", Some("e")));
        assert_ne!(base, fingerprint("r", "f", "x", None));
    }

    #[test]
    fn disambiguate_numbers_equal_fingerprints() {
        let r = |line| {
            SarifResult::new("r", Level::Note, "m", "f", Region::at(line, 1))
                .with_fingerprint("same", None)
        };
        let mut list = vec![r(1), r(2), r(3)];
        disambiguate(&mut list);
        let fps: Vec<_> = list
            .iter()
            .map(|r| r.fingerprint().unwrap().to_owned())
            .collect();
        assert_eq!(fps[1], format!("{}#1", fps[0]));
        assert_eq!(fps[2], format!("{}#2", fps[0]));
    }

    #[test]
    fn log_round_trips_and_reads_standard_sarif() {
        let mut run = Run::new("aivory-rules");
        run.results.push(
            SarifResult::new(
                "aivory/long-function",
                Level::Warning,
                "too long",
                "a.rs",
                Region::span(1, 1, 90, 2),
            )
            .with_fingerprint("fn a() {", Some("a")),
        );
        let log = SarifLog {
            runs: vec![run],
            ..SarifLog::default()
        };
        let text = serde_json::to_string(&log).unwrap();
        assert!(text.contains("\"ruleId\":\"aivory/long-function\""));
        assert!(text.contains("\"$schema\""));
        let back: SarifLog = serde_json::from_str(&text).unwrap();
        assert_eq!(back, log);
        // Foreign SARIF with extra fields.
        let foreign = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"x","informationUri":"u"}},
            "results":[{"ruleId":"r","level":"error","message":{"text":"t"},"ruleIndex":0,
            "locations":[{"physicalLocation":{"artifactLocation":{"uri":"f","index":0},"region":{"startLine":2}}}]}]}]}"#;
        let log: SarifLog = serde_json::from_str(foreign).unwrap();
        assert_eq!(log.result_count(), 1);
        assert_eq!(log.results().next().unwrap().level, Level::Error);
    }
}
