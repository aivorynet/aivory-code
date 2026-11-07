//! Compare the findings before and after a turn.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::sarif::{Level, SarifResult};

/// The difference between two lists of results, matched by fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    /// In `after`, not in `before`.
    pub added: Vec<SarifResult>,
    /// In `before`, not in `after`.
    pub removed: Vec<SarifResult>,
    /// In both lists. The entries come from `after`.
    pub unchanged: Vec<SarifResult>,
}

/// The numbers that the focus layer uses to score a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreInput {
    /// New results with the level `error`. The score of a turn goes up with this number.
    pub errors_added: u32,
    pub warnings_added: u32,
    pub errors_removed: u32,
    pub warnings_removed: u32,
}

impl Diff {
    pub fn score_input(&self) -> ScoreInput {
        let count = |list: &[SarifResult], level: Level| {
            list.iter().filter(|r| r.level == level).count() as u32
        };
        ScoreInput {
            errors_added: count(&self.added, Level::Error),
            warnings_added: count(&self.added, Level::Warning),
            errors_removed: count(&self.removed, Level::Error),
            warnings_removed: count(&self.removed, Level::Warning),
        }
    }

    /// The number of new results with the level `error`.
    pub fn errors_added(&self) -> u32 {
        self.score_input().errors_added
    }
}

/// The match key of a result: the fingerprint, or a fallback of rule, file, and message
/// for a result without a fingerprint (the line number does not take part).
fn key(r: &SarifResult) -> String {
    match r.fingerprint() {
        Some(fp) => format!("fp:{}:{fp}", r.rule_id),
        None => format!(
            "raw:{}:{}:{}",
            r.rule_id,
            r.file().unwrap_or(""),
            r.message.text
        ),
    }
}

/// Compare two lists of results. A result is the same in both lists when its fingerprint is the
/// same. If a key occurs `n` times before and `m` times after, `min(n, m)` results are
/// unchanged, and the others are added or removed.
pub fn diff<'a, B, A>(before: B, after: A) -> Diff
where
    B: IntoIterator<Item = &'a SarifResult>,
    A: IntoIterator<Item = &'a SarifResult>,
{
    let mut pool: HashMap<String, Vec<&SarifResult>> = HashMap::new();
    for r in before {
        pool.entry(key(r)).or_default().push(r);
    }
    let mut out = Diff::default();
    for r in after {
        match pool.get_mut(&key(r)).and_then(Vec::pop) {
            Some(_) => out.unchanged.push(r.clone()),
            None => out.added.push(r.clone()),
        }
    }
    let mut rest: Vec<&SarifResult> = pool.into_values().flatten().collect();
    rest.sort_by_key(|r| {
        (
            r.file().map(str::to_owned),
            r.start_line(),
            r.rule_id.clone(),
        )
    });
    out.removed = rest.into_iter().cloned().collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sarif::Region;

    fn res(rule: &str, level: Level, line: u32, text: &str) -> SarifResult {
        SarifResult::new(rule, level, "m", "a.rs", Region::at(line, 1))
            .with_fingerprint(text, Some("f"))
    }

    #[test]
    fn diff_splits_added_removed_unchanged() {
        let before = vec![
            res("r", Level::Error, 1, "one"),
            res("r", Level::Warning, 2, "two"),
        ];
        let after = vec![
            // "two" moved from line 2 to line 30: unchanged.
            res("r", Level::Warning, 30, "two"),
            res("r", Level::Error, 5, "three"),
            res("r", Level::Warning, 6, "four"),
        ];
        let d = diff(&before, &after);
        assert_eq!(d.unchanged.len(), 1);
        assert_eq!(d.unchanged[0].start_line(), Some(30));
        assert_eq!(d.added.len(), 2);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].level, Level::Error);
        let s = d.score_input();
        assert_eq!(
            s,
            ScoreInput {
                errors_added: 1,
                warnings_added: 1,
                errors_removed: 1,
                warnings_removed: 0
            }
        );
        assert_eq!(d.errors_added(), 1);
    }

    #[test]
    fn equal_lists_have_no_difference() {
        let list = vec![
            res("r", Level::Error, 1, "x"),
            res("q", Level::Note, 2, "y"),
        ];
        let d = diff(&list, &list);
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert_eq!(d.unchanged.len(), 2);
    }

    #[test]
    fn results_without_fingerprint_match_by_rule_file_message() {
        let a = SarifResult::new("r", Level::Error, "m", "a.rs", Region::at(1, 1));
        let b = SarifResult::new("r", Level::Error, "m", "a.rs", Region::at(9, 1));
        let d = diff([&a], [&b]);
        assert_eq!(d.unchanged.len(), 1);
    }
}
