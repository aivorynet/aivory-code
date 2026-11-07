use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

use crate::cache::{Entry, ScanCache};
use crate::claude::ClaudeStore;
use crate::codex::CodexStore;
use crate::gemini::GeminiStore;
use crate::model::{Provider, SessionFilter, SessionSummary};
use crate::roots::StoreRoots;
use crate::store::{FileRole, Parsed, SessionFile, SessionStore};

/// Counters of one scan.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanStats {
    /// Candidate files found, including sidechain files.
    pub files: usize,
    pub cache_hits: usize,
    /// Files that were opened and parsed.
    pub parsed: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    /// All sessions, newest first.
    pub sessions: Vec<SessionSummary>,
    pub stats: ScanStats,
}

#[derive(Debug, thiserror::Error)]
pub enum FindError {
    #[error("no session matches '{0}'")]
    NotFound(String),
    #[error("'{query}' matches {} sessions; use a longer id or --provider", .candidates.len())]
    Ambiguous {
        query: String,
        candidates: Vec<SessionSummary>,
    },
}

/// Index over the session stores.
pub struct SessionCatalog {
    stores: Vec<Box<dyn SessionStore>>,
    cache_file: Option<PathBuf>,
}

impl std::fmt::Debug for SessionCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCatalog")
            .field("stores", &self.stores.len())
            .field("cache_file", &self.cache_file)
            .finish()
    }
}

impl SessionCatalog {
    /// Catalog over the three native stores below `roots`. No cache until [`Self::with_cache_file`].
    pub fn new(roots: &StoreRoots) -> Self {
        Self::with_stores(vec![
            Box::new(ClaudeStore::new(&roots.claude)),
            Box::new(CodexStore::new(&roots.codex)),
            Box::new(GeminiStore::new(&roots.gemini)),
        ])
    }

    /// Catalog over custom stores.
    pub fn with_stores(stores: Vec<Box<dyn SessionStore>>) -> Self {
        Self {
            stores,
            cache_file: None,
        }
    }

    /// Keep parse results in this file between scans. `None` disables the cache.
    #[must_use]
    pub fn with_cache_file(mut self, path: Option<PathBuf>) -> Self {
        self.cache_file = path;
        self
    }

    /// Scan all stores. Unchanged files come from the cache.
    pub fn scan(&self) -> ScanReport {
        let started = Instant::now();
        let cache = self
            .cache_file
            .as_deref()
            .map(ScanCache::load)
            .unwrap_or_default();

        let files: Vec<(usize, SessionFile)> = self
            .stores
            .par_iter()
            .enumerate()
            .flat_map_iter(|(i, s)| s.enumerate().into_iter().map(move |f| (i, f)))
            .collect();

        let hits = AtomicUsize::new(0);
        let parsed_count = AtomicUsize::new(0);
        let results: Vec<(usize, SessionFile, Option<Parsed>, bool)> = files
            .into_par_iter()
            .map(|(i, file)| {
                if let FileRole::Sidechain { parent_id: Some(p) } = &file.role {
                    // Parent known from the path: nothing to read, nothing to cache.
                    let parsed = Some(Parsed::Sidechain {
                        parent_id: p.clone(),
                    });
                    return (i, file, parsed, false);
                }
                if let Some(cached) = cache.get(&file) {
                    hits.fetch_add(1, Ordering::Relaxed);
                    return (i, file, cached.clone(), true);
                }
                parsed_count.fetch_add(1, Ordering::Relaxed);
                let parsed = self.stores[i].parse(&file);
                (i, file, parsed, true)
            })
            .collect();

        let total_files = results.len();
        let mut sessions: HashMap<(Provider, String), SessionSummary> = HashMap::new();
        let mut sidechains: HashMap<(Provider, String), Vec<PathBuf>> = HashMap::new();
        let mut new_entries = Vec::new();
        for (i, file, parsed, cacheable) in results {
            let provider = self.stores[i].provider();
            if cacheable {
                new_entries.push(Entry {
                    path: file.path.clone(),
                    mtime_ms: file.mtime_ms,
                    size: file.size,
                    parsed: parsed.clone(),
                });
            }
            match parsed {
                Some(Parsed::Session(mut s)) => {
                    if s.cwd.is_none() {
                        s.cwd = file.cwd_hint.clone();
                    }
                    let key = (s.provider, s.id.clone());
                    match sessions.get(&key) {
                        Some(old) if old.updated >= s.updated => {}
                        _ => {
                            sessions.insert(key, s);
                        }
                    }
                }
                Some(Parsed::Sidechain { parent_id }) => sidechains
                    .entry((provider, parent_id))
                    .or_default()
                    .push(file.path),
                None => {}
            }
        }
        for (key, mut paths) in sidechains {
            if let Some(s) = sessions.get_mut(&key) {
                paths.sort();
                s.sidechains = paths;
            }
        }
        let mut sessions: Vec<SessionSummary> = sessions.into_values().collect();
        sessions.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));

        let parsed = parsed_count.load(Ordering::Relaxed);
        if let Some(path) = &self.cache_file
            && (parsed > 0 || new_entries.len() != cache.len())
        {
            ScanCache::save(path, new_entries);
        }
        ScanReport {
            sessions,
            stats: ScanStats {
                files: total_files,
                cache_hits: hits.load(Ordering::Relaxed),
                parsed,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
        }
    }

    /// Scan, then filter. Newest first.
    pub fn list(&self, filter: &SessionFilter) -> Vec<SessionSummary> {
        let mut sessions: Vec<SessionSummary> = self
            .scan()
            .sessions
            .into_iter()
            .filter(|s| filter.matches(s))
            .collect();
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        sessions
    }

    /// Find one session by full id or by a unique id prefix.
    pub fn find(
        &self,
        query: &str,
        provider: Option<Provider>,
    ) -> Result<SessionSummary, FindError> {
        let all = self.scan().sessions;
        let in_scope = |s: &&SessionSummary| provider.is_none_or(|p| p == s.provider);
        let exact: Vec<&SessionSummary> = all
            .iter()
            .filter(in_scope)
            .filter(|s| s.id == query)
            .collect();
        let found = if exact.is_empty() {
            all.iter()
                .filter(in_scope)
                .filter(|s| !query.is_empty() && s.id.starts_with(query))
                .collect()
        } else {
            exact
        };
        match found.as_slice() {
            [] => Err(FindError::NotFound(query.to_string())),
            [one] => Ok((*one).clone()),
            many => Err(FindError::Ambiguous {
                query: query.to_string(),
                candidates: many.iter().map(|s| (*s).clone()).collect(),
            }),
        }
    }
}
