//! Helpers for the integration tests: the fake language server and the pools.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aivory_diag::lsp::{
    Lineage, LspPool, MemoryProbe, Overlay, OverlayFile, PoolConfig, ServerSpec,
};
use aivory_syntax::Language;

pub fn fake_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-lsp"))
}

/// A spec for the fake server. `log` is a file that receives the received methods.
pub fn fake_spec(log: Option<&Path>) -> ServerSpec {
    let mut spec = ServerSpec::new("fake", "fake-lsp", &[], &[Language::Rust]);
    spec.exe = Some(fake_exe());
    if let Some(log) = log {
        spec.args = vec!["--log".into(), log.to_string_lossy().into_owned()];
    }
    spec
}

/// Every pid has the same memory.
#[derive(Debug)]
pub struct FixedProbe(pub u64);

impl MemoryProbe for FixedProbe {
    fn sample(&self, pids: &[u32]) -> HashMap<u32, u64> {
        pids.iter().map(|p| (*p, self.0)).collect()
    }
}

/// The memory of each pid can be set.
#[derive(Debug, Default)]
pub struct MapProbe(pub Mutex<HashMap<u32, u64>>);

impl MemoryProbe for MapProbe {
    fn sample(&self, pids: &[u32]) -> HashMap<u32, u64> {
        let m = self.0.lock().unwrap();
        pids.iter()
            .map(|p| (*p, m.get(p).copied().unwrap_or(0)))
            .collect()
    }
}

pub fn config() -> PoolConfig {
    PoolConfig {
        memory_cap_bytes: u64::MAX,
        start_timeout: Duration::from_secs(20),
        request_timeout: Duration::from_secs(10),
        diagnostics_wait: Duration::from_secs(5),
        ..PoolConfig::default()
    }
}

pub fn fake_pool(log: Option<&Path>) -> LspPool {
    LspPool::new(config(), vec![fake_spec(log)], Arc::new(FixedProbe(1)))
}

/// A lineage in a temp folder with a file `src/lib.rs` on disk.
pub fn lineage(dir: &Path, id: &str) -> Lineage {
    let root = dir.join(id);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src").join("lib.rs"),
        "disk line 0\ndisk line 1\n",
    )
    .unwrap();
    Lineage {
        id: id.to_owned(),
        root,
    }
}

pub fn overlay(session: &str, files: &[(&str, &str)]) -> Overlay {
    Overlay::new(
        session,
        files
            .iter()
            .map(|(p, t)| OverlayFile {
                path: (*p).to_owned(),
                text: (*t).to_owned(),
            })
            .collect(),
    )
}

pub fn read_log(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

pub fn count(log: &[String], method: &str) -> usize {
    log.iter()
        .filter(|l| l.starts_with(&format!("{method} ")))
        .count()
}
