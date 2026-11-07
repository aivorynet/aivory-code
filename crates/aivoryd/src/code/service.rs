//! The methods `code/*` and `ledger/*`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use aivory_diag::lsp::lsp_types::{Position, Range as LspRange};
use aivory_diag::lsp::{Ctx, Overlay, OverlayFile};
use aivory_index::{DefHit, PageReq, RefHit};
use aivory_protocol::code::{
    self as api, BlameHunk, BlameParams, BlameTurn, CodeFile, CodeFileParams, CodePosParams,
    CodeSource, CommitRef, DiagnosticsParams, DiagnosticsResult, FileBlame, FileEdits, Finding,
    FindingLevel, Fold, FoldKind, Highlight, Hover, LedgerTurn, LedgerTurnsParams, LineRange,
    OutlineNode, Place, PlacePage, RenameParams, RenamePreview, RevertParams, RevertResult, Symbol,
    SymbolKind, SymbolPage, SymbolsParams, TextEdit, Timeline, TimelineEvent, TimelineParams,
    TokenKind, TurnFileStat, TurnId, TurnList,
};
use aivory_protocol::rpc::{RpcError, code};
use aivory_syntax::{DeclKind, Document, Language};
use aivory_vcs::{GitVcs, Ledger, Vcs, VcsError, blame_by_prompt, file_timeline, revert_turn};
use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::demo::DemoCode;
use super::diag;
use super::resolve::{Resolver, SessionCtx};
use super::state::{CodeCore, IndexEntry};
use crate::ledger::cut;

/// The longest text of `code/file`, in bytes.
const MAX_TEXT: usize = 1 << 20;
/// A file larger than this is not read.
const MAX_READ: u64 = 8 << 20;
/// The lines that `code/file` highlights when the request names none.
const DEFAULT_LINES: u32 = 200;
/// How long a query waits for the first refresh of an index.
const INDEX_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_PER_PAGE: u32 = 200;
/// The most edits that a rename preview holds.
const MAX_EDITS: usize = 5_000;

type Res<T> = Result<T, RpcError>;

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn server(msg: impl std::fmt::Display) -> RpcError {
    RpcError::new(code::SERVER_ERROR, msg.to_string())
}

fn vcs_error(e: VcsError) -> RpcError {
    match e {
        VcsError::NotFound(m) => RpcError::new(code::NOT_FOUND, m),
        VcsError::Invalid(m) => invalid(m),
        other => server(other),
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Res<T> {
    let params = if params.is_null() {
        Value::Object(Default::default())
    } else {
        params
    };
    T::deserialize(params).map_err(|e| invalid(format!("invalid params: {e}")))
}

fn to_value<T: serde::Serialize>(v: &T) -> Res<Value> {
    serde_json::to_value(v).map_err(server)
}

/// The service. A live service has a core and a resolver; a demo service has both too (over
/// scratch files) and answers the ledger from the demo data.
#[derive(Debug)]
pub struct CodeService {
    core: Option<Arc<CodeCore>>,
    resolver: Option<Arc<dyn Resolver>>,
    demo: Option<DemoCode>,
}

impl CodeService {
    pub fn live(core: Arc<CodeCore>, resolver: Arc<dyn Resolver>) -> Self {
        Self {
            core: Some(core),
            resolver: Some(resolver),
            demo: None,
        }
    }

    /// A service on the synthetic files of the demo codebases.
    pub fn demo() -> Self {
        let demo = DemoCode::new();
        Self {
            core: Some(demo.core()),
            resolver: Some(demo.resolver()),
            demo: Some(demo),
        }
    }

    pub fn core(&self) -> Option<&Arc<CodeCore>> {
        self.core.as_ref()
    }

    pub fn resolver(&self) -> Option<&Arc<dyn Resolver>> {
        self.resolver.as_ref()
    }

    /// Answer one method.
    pub fn dispatch(&self, method: &str, params: Value) -> Res<Value> {
        match method {
            "code/file" => to_value(&self.file(parse(params)?)?),
            "code/definition" => to_value(&self.definition(parse(params)?)?),
            "code/references" => to_value(&self.references(parse(params)?)?),
            "code/hover" => to_value(&self.hover(parse(params)?)?),
            "code/symbols" => to_value(&self.symbols(parse(params)?)?),
            "code/diagnostics" => to_value(&self.diagnostics(parse(params)?)?),
            "code/rename" => to_value(&self.rename(parse(params)?)?),
            "ledger/turns" => to_value(&self.turns(parse(params)?)?),
            "ledger/blame" => to_value(&self.blame(parse(params)?)?),
            "ledger/timeline" => to_value(&self.timeline(parse(params)?)?),
            "ledger/revert" => to_value(&self.revert(parse(params)?)?),
            other => Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("unknown method '{other}'"),
            )),
        }
    }

    fn core_ref(&self) -> Res<&Arc<CodeCore>> {
        self.core
            .as_ref()
            .ok_or_else(|| RpcError::new(code::NOT_AVAILABLE, "the daemon has no code service"))
    }

    fn resolver_ref(&self) -> Res<&Arc<dyn Resolver>> {
        self.resolver
            .as_ref()
            .ok_or_else(|| RpcError::new(code::NOT_AVAILABLE, "the daemon has no code service"))
    }

    // -----------------------------------------------------------------------------------------
    // Targets

    fn session_ctx(&self, id: &str) -> Res<SessionCtx> {
        self.resolver_ref()?
            .session(id)
            .ok_or_else(|| RpcError::new(code::SESSION_NOT_FOUND, format!("no session '{id}'")))
    }

    fn target(&self, codebase: &str, session: Option<&str>) -> Res<Target> {
        let root = self
            .resolver_ref()?
            .codebase_root(codebase)
            .ok_or_else(|| RpcError::not_found("codebase", codebase))?;
        let ctx = session.map(|s| self.session_ctx(s)).transpose()?;
        let workdir = ctx
            .as_ref()
            .filter(|c| c.codebase.as_deref().is_none_or(|b| b == codebase))
            .and_then(|c| c.workdir.clone())
            .filter(|w| w.is_dir())
            .unwrap_or_else(|| root.clone());
        Ok(Target {
            codebase: codebase.to_owned(),
            root,
            workdir,
            native: ctx.map(|c| c.native),
        })
    }

    fn entry(&self, t: &Target) -> Res<Arc<IndexEntry>> {
        let entry = self
            .core_ref()?
            .entry(&t.root, &t.codebase)
            .map_err(server)?;
        entry.wait_ready(INDEX_WAIT);
        Ok(entry)
    }

    // -----------------------------------------------------------------------------------------
    // code/file

    fn file(&self, p: CodeFileParams) -> Res<CodeFile> {
        let t = self.target(&p.codebase, p.session.as_deref())?;
        let rel = safe_rel(&p.path)?;
        let (text, truncated) = cut_text(self.read_text(&t, &rel, p.rev.as_deref())?);
        if p.rev.is_none() {
            self.core_ref()?.warm_lsp(&t.root, &rel);
        }
        let total_lines = count_lines(&text);
        let from = p.lines.map_or(1, |l| l.from.max(1));
        let to = p
            .lines
            .map_or(DEFAULT_LINES, |l| l.to)
            .min(total_lines)
            .max(from.saturating_sub(1));
        let mut file = CodeFile {
            path: rel.clone(),
            language: String::new(),
            total_lines,
            truncated,
            highlighted: LineRange { from, to },
            highlights: Vec::new(),
            outline: Vec::new(),
            folding: Vec::new(),
            diagnostics: Vec::new(),
            text,
        };
        let mut syntax_rows = Vec::new();
        if let Some(language) = Language::detect(&rel)
            && let Ok(doc) = Document::new(language, file.text.clone())
        {
            file.language = language.name().to_owned();
            if to >= from
                && let Ok(spans) = doc.highlights_in_lines((from - 1) as usize, to as usize)
            {
                file.highlights = highlight_rows(&file.text, &spans, from, to);
            }
            if let Ok(stub) = doc.stub() {
                file.outline = stub.outline.iter().map(outline_node).collect();
                file.folding = stub.folding.iter().map(fold).collect();
            }
            syntax_rows = doc
                .syntax_errors()
                .iter()
                .map(|e| Finding {
                    rule: aivory_diag::RULE_SYNTAX_ERROR.to_owned(),
                    level: FindingLevel::Error,
                    message: e.message.clone(),
                    path: rel.clone(),
                    line: e.range.start_line + 1,
                    col: e.range.start_col,
                    end_line: Some(e.range.end_line + 1),
                    end_col: Some(e.range.end_col),
                    tool: "aivory-syntax".to_owned(),
                    fingerprint: String::new(),
                })
                .collect();
        }
        let stored = match (&t.native, &p.rev) {
            (Some(native), None) => {
                diag::latest_for_file(self.core_ref()?.db(), native, &rel).map_err(server)?
            }
            _ => Vec::new(),
        };
        file.diagnostics = if stored.is_empty() {
            syntax_rows
        } else {
            stored
        };
        Ok(file)
    }

    /// The text of a file: the working tree of the target, or a revision.
    fn read_text(&self, t: &Target, rel: &str, rev: Option<&str>) -> Res<String> {
        let bytes = match rev {
            None => {
                let path = on_disk(&t.workdir, rel)?;
                let len = std::fs::metadata(&path).map_err(server)?.len();
                if len > MAX_READ {
                    return Err(invalid(format!("'{rel}' is larger than {MAX_READ} bytes")));
                }
                std::fs::read(&path).map_err(server)?
            }
            Some(rev) => {
                let commit = self.revision(t, rev)?;
                GitVcs::new()
                    .show_file(&t.workdir, &commit, rel)
                    .map_err(vcs_error)?
            }
        };
        String::from_utf8(bytes).map_err(|_| invalid(format!("'{rel}' is not a UTF-8 text file")))
    }

    /// A git revision, or `turn:<n>` as the snapshot commit of that turn.
    fn revision(&self, t: &Target, rev: &str) -> Res<String> {
        let Some(n) = rev.strip_prefix("turn:") else {
            if rev.starts_with('-') || rev.is_empty() {
                return Err(invalid("bad revision"));
            }
            return Ok(rev.to_owned());
        };
        let n: u32 = n.parse().map_err(|_| invalid("bad turn number"))?;
        let native = t
            .native
            .as_deref()
            .ok_or_else(|| invalid("a revision 'turn:<n>' needs a session"))?;
        let snaps = GitVcs::new()
            .list_snapshots(&t.workdir, native)
            .map_err(vcs_error)?;
        snaps
            .into_iter()
            .rev()
            .find(|s| s.n <= n)
            .map(|s| s.commit)
            .ok_or_else(|| RpcError::new(code::NOT_FOUND, format!("no snapshot of turn {n}")))
    }

    // -----------------------------------------------------------------------------------------
    // Position queries

    fn cursor(&self, p: &CodePosParams) -> Res<Cursor> {
        let t = self.target(&p.codebase, p.session.as_deref())?;
        let rel = safe_rel(&p.path)?;
        let text = self.read_text(&t, &rel, None)?;
        let line_text = text
            .lines()
            .nth((p.line as usize).saturating_sub(1))
            .ok_or_else(|| invalid("the line is past the end of the file"))?
            .to_owned();
        let (start, end) = word_at(&line_text, p.col as usize).unwrap_or((0, 0));
        let name = line_text[start..end].to_owned();
        Ok(Cursor {
            t,
            rel,
            line: p.line,
            col: (p.col as usize).min(line_text.len()),
            word: (start, end),
            name,
            line_text,
            page: p.page.unwrap_or(1).max(1),
            per_page: p.per_page.unwrap_or(20).clamp(1, MAX_PER_PAGE),
        })
    }

    fn definition(&self, p: CodePosParams) -> Res<PlacePage> {
        let c = self.cursor(&p)?;
        if c.name.is_empty() {
            return Ok(c.empty_page());
        }
        if let Some(places) = self.lsp_places(&c, Lsp::Definition) {
            return Ok(c.page_of(places, CodeSource::Lsp, false));
        }
        let entry = self.entry(&c.t)?;
        let page = entry
            .index
            .definition(
                c.t.native.as_deref(),
                &c.name,
                PageReq::new(c.page as usize, c.per_page as usize),
            )
            .map_err(server)?;
        Ok(PlacePage {
            name: c.name.clone(),
            total: page.total as u32,
            page: c.page,
            per_page: c.per_page,
            truncated: page.truncated,
            items: page.items.iter().map(place_of_def).collect(),
            source: CodeSource::Index,
            indexing: entry.indexing(),
        })
    }

    fn references(&self, p: CodePosParams) -> Res<PlacePage> {
        let c = self.cursor(&p)?;
        if c.name.is_empty() {
            return Ok(c.empty_page());
        }
        if let Some(places) = self.lsp_places(&c, Lsp::References) {
            return Ok(c.page_of(places, CodeSource::Lsp, false));
        }
        let entry = self.entry(&c.t)?;
        let page = entry
            .index
            .references(
                c.t.native.as_deref(),
                &c.name,
                PageReq::new(c.page as usize, c.per_page as usize),
            )
            .map_err(server)?;
        let mut files = Files::new(&c.t.workdir);
        Ok(PlacePage {
            name: c.name.clone(),
            total: page.total as u32,
            page: c.page,
            per_page: c.per_page,
            truncated: page.truncated,
            items: page
                .items
                .iter()
                .map(|h| place_of_ref(h, &mut files))
                .collect(),
            source: CodeSource::Index,
            indexing: entry.indexing(),
        })
    }

    fn hover(&self, p: CodePosParams) -> Res<Hover> {
        let c = self.cursor(&p)?;
        if c.name.is_empty() {
            return Ok(Hover {
                name: String::new(),
                text: String::new(),
                signature: None,
                kind: None,
                range: None,
                definitions: Vec::new(),
                source: CodeSource::Syntax,
                indexing: false,
            });
        }
        let range = Highlight {
            line: c.line,
            start: c.word.0 as u32,
            end: c.word.1 as u32,
            kind: TokenKind::Variable,
        };
        let lsp_text = self.lsp_hover(&c);
        let entry = self.entry(&c.t)?;
        let defs = entry
            .index
            .definition(c.t.native.as_deref(), &c.name, PageReq::new(1, 5))
            .map_err(server)?;
        let first = defs.items.first();
        let language = Language::detect(&c.rel).map_or("", Language::name);
        let signature = first.map(|d| d.signature.clone());
        let mut text = String::new();
        if let Some(sig) = &signature {
            text.push_str(&format!("```{language}\n{sig}\n```\n"));
        }
        let source = match lsp_text {
            Some(doc) => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&doc);
                CodeSource::Lsp
            }
            None => CodeSource::Index,
        };
        if let Some(d) = first {
            let kind = kind_text(decl_kind(d.kind));
            text.push_str(&format!("\n{kind} in `{}:{}`", d.path, d.line));
        }
        Ok(Hover {
            name: c.name.clone(),
            text,
            signature,
            kind: first.map(|d| decl_kind(d.kind)),
            range: Some(range),
            definitions: defs.items.iter().map(place_of_def).collect(),
            source,
            indexing: entry.indexing(),
        })
    }

    // -----------------------------------------------------------------------------------------
    // Language servers

    /// The overlay of a session in its own worktree: the files that it changed.
    fn overlay(&self, c: &Cursor, entry: Option<&Arc<IndexEntry>>) -> Overlay {
        let (Some(native), Some(entry)) = (c.t.native.as_deref(), entry) else {
            return Overlay::default();
        };
        if c.t.workdir == c.t.root {
            return Overlay::default();
        }
        let paths = entry
            .index
            .session_paths(native)
            .map(|(changed, _)| changed)
            .unwrap_or_default();
        let files = paths
            .into_iter()
            .take(50)
            .filter_map(|path| {
                let text = std::fs::read_to_string(c.t.workdir.join(&path)).ok()?;
                Some(OverlayFile { path, text })
            })
            .collect();
        Overlay::new(native, files)
    }

    /// The places that a running language server gives. `None`: no server runs, it failed,
    /// or it found nothing (the index answers then).
    fn lsp_places(&self, c: &Cursor, query: Lsp) -> Option<Vec<Place>> {
        let core = self.core.as_ref()?;
        let language = Language::detect(&c.rel)?;
        let lineage = CodeCore::lineage(&c.t.root);
        if !core.pool().is_running(&lineage, language) {
            return None;
        }
        let entry = core.entry(&c.t.root, &c.t.codebase).ok();
        let overlay = self.overlay(c, entry.as_ref());
        let ctx = Ctx {
            lineage: &lineage,
            overlay: &overlay,
        };
        let pos = Position::new(c.line - 1, utf16_col(&c.line_text, c.col));
        let places = match query {
            Lsp::Definition => core.pool().definition(&ctx, &c.rel, pos),
            Lsp::References => core.pool().references(&ctx, &c.rel, pos),
        }
        .ok()?;
        if places.is_empty() {
            return None;
        }
        let mut files = Files::new(&c.t.root);
        Some(
            places
                .into_iter()
                .map(|pl| {
                    let (line, col, len) = files.start_of(&pl.path, &pl.range);
                    Place {
                        preview: files.line(&pl.path, line),
                        path: pl.path,
                        line,
                        col,
                        len,
                        name: Some(c.name.clone()),
                        kind: None,
                        container: None,
                    }
                })
                .collect(),
        )
    }

    fn lsp_hover(&self, c: &Cursor) -> Option<String> {
        let core = self.core.as_ref()?;
        let language = Language::detect(&c.rel)?;
        let lineage = CodeCore::lineage(&c.t.root);
        if !core.pool().is_running(&lineage, language) {
            return None;
        }
        let overlay = Overlay::default();
        let ctx = Ctx {
            lineage: &lineage,
            overlay: &overlay,
        };
        let pos = Position::new(c.line - 1, utf16_col(&c.line_text, c.col));
        let info = core.pool().hover(&ctx, &c.rel, pos).ok()??;
        Some(info.text).filter(|t| !t.trim().is_empty())
    }

    // -----------------------------------------------------------------------------------------
    // code/symbols, code/diagnostics, code/rename

    fn symbols(&self, p: SymbolsParams) -> Res<SymbolPage> {
        let t = self.target(&p.codebase, p.session.as_deref())?;
        let entry = self.entry(&t)?;
        let page = p.page.unwrap_or(1).max(1);
        let per_page = p.per_page.unwrap_or(20).clamp(1, MAX_PER_PAGE);
        let result = entry
            .index
            .symbols(
                t.native.as_deref(),
                &p.query,
                p.kind.map(kind_to_decl),
                PageReq::new(page as usize, per_page as usize),
            )
            .map_err(server)?;
        Ok(SymbolPage {
            total: result.total as u32,
            page,
            per_page,
            truncated: result.truncated,
            indexing: entry.indexing(),
            items: result
                .items
                .into_iter()
                .map(|h| Symbol {
                    name: h.name,
                    kind: decl_kind(h.kind),
                    path: h.path,
                    line: h.line,
                    container: h.container,
                    signature: h.signature,
                })
                .collect(),
        })
    }

    fn diagnostics(&self, p: DiagnosticsParams) -> Res<DiagnosticsResult> {
        let t = self.target(&p.codebase, p.session.as_deref())?;
        let path = p.path.as_deref().map(safe_rel).transpose()?;
        if let Some(demo) = &self.demo {
            return Ok(demo.diagnostics(&p, path.as_deref()));
        }
        let core = self.core_ref()?;
        let Some(native) = t.native.as_deref() else {
            if p.turn.is_some() {
                return Err(invalid("'turn' needs a session"));
            }
            // No session: the fast stages on the files that changed in the checkout.
            let files = match &path {
                Some(f) => vec![f.clone()],
                None => diag::changed_paths(&t.workdir),
            };
            let (rows, mut notes) = diag::quick(&t.workdir, &files);
            notes.push("no session: the findings are not compared with an earlier state".into());
            return Ok(DiagnosticsResult {
                turn: None,
                rows,
                added: Vec::new(),
                removed: Vec::new(),
                notes,
            });
        };
        let stored = diag::load(core.db(), native, p.turn).map_err(server)?;
        let Some(s) = stored else {
            if let Some(n) = p.turn {
                return Err(RpcError::new(
                    code::NOT_FOUND,
                    format!("no stored findings for turn {n}"),
                ));
            }
            return Ok(DiagnosticsResult {
                turn: None,
                rows: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
                notes: vec!["no turn of this session has stored findings yet".into()],
            });
        };
        let keep = |f: &Finding| path.as_deref().is_none_or(|p| f.path == p);
        Ok(DiagnosticsResult {
            turn: Some(s.n),
            rows: s.rows.into_iter().filter(keep).collect(),
            added: s.added.into_iter().filter(keep).collect(),
            removed: s.removed.into_iter().filter(keep).collect(),
            notes: s.notes,
        })
    }

    fn rename(&self, p: RenameParams) -> Res<RenamePreview> {
        if !valid_identifier(&p.new_name) {
            return Err(invalid(format!("'{}' is not a valid name", p.new_name)));
        }
        let c = self.cursor(&CodePosParams {
            codebase: p.codebase.clone(),
            path: p.path.clone(),
            line: p.line,
            col: p.col,
            session: p.session.clone(),
            page: None,
            per_page: None,
        })?;
        if c.name.is_empty() {
            return Err(invalid("there is no name at the cursor"));
        }
        if let Some(files) = self.lsp_rename(&c, &p.new_name) {
            return Ok(RenamePreview {
                old_name: c.name.clone(),
                new_name: p.new_name,
                files,
                exact: true,
                source: CodeSource::Lsp,
            });
        }
        let entry = self.entry(&c.t)?;
        let native = c.t.native.as_deref();
        let mut sites: Vec<(String, u32, u32, u32)> = Vec::new();
        let mut page = 1;
        loop {
            let defs = entry
                .index
                .definition(native, &c.name, PageReq::new(page, 200))
                .map_err(server)?;
            let more = defs.items.len() == 200;
            for d in defs.items.iter().filter(|d| d.name == c.name) {
                sites.push((
                    d.path.clone(),
                    d.line,
                    d.col.saturating_sub(1),
                    c.name.len() as u32,
                ));
            }
            if !more || sites.len() >= MAX_EDITS {
                break;
            }
            page += 1;
        }
        let mut page = 1;
        loop {
            let refs = entry
                .index
                .references(native, &c.name, PageReq::new(page, 200))
                .map_err(server)?;
            let more = refs.items.len() == 200;
            for r in &refs.items {
                sites.push((
                    r.path.clone(),
                    r.line,
                    r.col.saturating_sub(1),
                    r.end_byte - r.start_byte,
                ));
            }
            if !more || sites.len() >= MAX_EDITS {
                break;
            }
            page += 1;
        }
        if sites.is_empty() {
            return Err(invalid(format!("the index has no use of '{}'", c.name)));
        }
        sites.sort();
        sites.dedup();
        let mut files: Vec<FileEdits> = Vec::new();
        for (path, line, col, len) in sites {
            let edit = TextEdit {
                line,
                col,
                end_line: line,
                end_col: col + len,
                new_text: p.new_name.clone(),
            };
            match files.last_mut() {
                Some(f) if f.path == path => f.edits.push(edit),
                _ => files.push(FileEdits {
                    path,
                    edits: vec![edit],
                }),
            }
        }
        Ok(RenamePreview {
            old_name: c.name.clone(),
            new_name: p.new_name,
            files,
            exact: false,
            source: CodeSource::Index,
        })
    }

    fn lsp_rename(&self, c: &Cursor, new_name: &str) -> Option<Vec<FileEdits>> {
        let core = self.core.as_ref()?;
        let language = Language::detect(&c.rel)?;
        let lineage = CodeCore::lineage(&c.t.root);
        if !core.pool().is_running(&lineage, language) {
            return None;
        }
        let overlay = Overlay::default();
        let ctx = Ctx {
            lineage: &lineage,
            overlay: &overlay,
        };
        let pos = Position::new(c.line - 1, utf16_col(&c.line_text, c.col));
        let edits = core.pool().rename(&ctx, &c.rel, pos, new_name).ok()?;
        if edits.is_empty() {
            return None;
        }
        let mut files = Files::new(&c.t.root);
        Some(
            edits
                .into_iter()
                .map(|f| FileEdits {
                    edits: f
                        .edits
                        .iter()
                        .map(|e| {
                            let (line, col, _) = files.start_of(&f.path, &e.range);
                            let (end_line, end_col, _) = files.end_of(&f.path, &e.range);
                            TextEdit {
                                line,
                                col,
                                end_line,
                                end_col,
                                new_text: e.new_text.clone(),
                            }
                        })
                        .collect(),
                    path: f.path,
                })
                .collect(),
        )
    }

    // -----------------------------------------------------------------------------------------
    // ledger/*

    fn ledger<T>(&self, f: impl FnOnce(&Ledger<'_>) -> Result<T, VcsError>) -> Res<T> {
        let core = self.core_ref()?;
        let conn = Connection::open(core.db()).map_err(server)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(server)?;
        let ledger = crate::ledger::open_ledger(&conn).map_err(vcs_error)?;
        f(&ledger).map_err(vcs_error)
    }

    fn turns(&self, p: LedgerTurnsParams) -> Res<TurnList> {
        if let Some(demo) = &self.demo {
            return demo.turns(&p.session);
        }
        let native = match self.resolver_ref()?.session(&p.session) {
            Some(c) => c.native,
            None => p.session.clone(),
        };
        let turns = self.ledger(|l| l.list_turns(&native))?;
        if turns.is_empty() && self.resolver_ref()?.session(&p.session).is_none() {
            return Err(RpcError::new(
                code::SESSION_NOT_FOUND,
                format!("no session '{}'", p.session),
            ));
        }
        Ok(TurnList {
            session: p.session,
            turns: turns.iter().map(ledger_turn).collect(),
        })
    }

    fn blame(&self, p: BlameParams) -> Res<FileBlame> {
        let t = self.target(&p.codebase, p.session.as_deref())?;
        let rel = safe_rel(&p.path)?;
        if let Some(demo) = &self.demo {
            return demo.blame(&rel);
        }
        on_disk(&t.workdir, &rel)?;
        let range = p.lines.map(|l| (l.from.max(1), l.to.max(l.from.max(1))));
        let vcs = GitVcs::new();
        let lines = self
            .ledger(|l| blame_by_prompt(&vcs, l, &t.workdir, &rel, range, t.native.as_deref()))?;
        let mut hunks: Vec<BlameHunk> = Vec::new();
        for l in lines {
            let commit = (!l.uncommitted).then(|| CommitRef {
                id: l.commit.chars().take(12).collect(),
                summary: l.summary.clone(),
                time: l.time * 1000,
            });
            let turn = match (&l.turn_ref, &l.turn) {
                (Some(r), Some(t)) => Some(BlameTurn {
                    session: r.session.clone(),
                    n: r.n,
                    prompt: cut(&t.prompt, api::PROMPT_CHARS),
                    model: t.model.clone().unwrap_or_default(),
                    time: t.ended_at * 1000,
                    cost: t.cost_usd,
                }),
                _ => None,
            };
            let same = |h: &BlameHunk| {
                h.commit.as_ref().map(|c| &c.id) == commit.as_ref().map(|c| &c.id)
                    && h.turn.as_ref().map(|t| (&t.session, t.n))
                        == turn.as_ref().map(|t| (&t.session, t.n))
                    && h.line + h.count == l.line
            };
            match hunks.last_mut() {
                Some(h) if same(h) => h.count += 1,
                _ => hunks.push(BlameHunk {
                    line: l.line,
                    count: 1,
                    commit,
                    turn,
                }),
            }
        }
        Ok(FileBlame { path: rel, hunks })
    }

    fn timeline(&self, p: TimelineParams) -> Res<Timeline> {
        let t = self.target(&p.codebase, p.session.as_deref())?;
        let rel = safe_rel(&p.path)?;
        if let Some(demo) = &self.demo {
            return demo.timeline(&rel);
        }
        let vcs = GitVcs::new();
        let events =
            self.ledger(|l| file_timeline(&vcs, l, &t.workdir, &rel, t.native.as_deref()))?;
        let events = events
            .into_iter()
            .map(|e| match e {
                aivory_vcs::TimelineEvent::Commit(c) => TimelineEvent::Commit {
                    id: c.id.chars().take(12).collect(),
                    summary: c.subject,
                    author: c.author_name,
                    time: c.time * 1000,
                    turns: c
                        .turns
                        .into_iter()
                        .map(|r| TurnId {
                            session: r.session,
                            n: r.n,
                        })
                        .collect(),
                },
                aivory_vcs::TimelineEvent::Turn(turn) => {
                    let stat = turn.files.iter().find(|f| f.path == rel);
                    TimelineEvent::Turn {
                        session: turn.session_id.clone(),
                        n: turn.n,
                        prompt: cut(&turn.prompt, api::PROMPT_CHARS),
                        model: turn.model.clone().unwrap_or_default(),
                        time: turn.ended_at * 1000,
                        cost: turn.cost_usd,
                        added: stat.map_or(0, |f| clamp_u32(f.added)),
                        removed: stat.map_or(0, |f| clamp_u32(f.removed)),
                    }
                }
            })
            .collect();
        Ok(Timeline { path: rel, events })
    }

    fn revert(&self, p: RevertParams) -> Res<RevertResult> {
        let ctx = self.session_ctx(&p.session)?;
        if let Some(demo) = &self.demo {
            return demo.revert(&p);
        }
        if p.turn == 0 {
            return Err(invalid("turn 0 is the baseline; there is nothing to undo"));
        }
        let workdir = ctx
            .workdir
            .clone()
            .filter(|w| w.is_dir())
            .ok_or_else(|| RpcError::new(code::INVALID_STATE, "the session has no folder"))?;
        // Only a linked worktree of the session: its `.git` is a file. The main checkout of
        // the user has a `.git` folder, and it is the root of the codebase.
        let main = workdir.join(".git").is_dir();
        let is_root = ctx
            .codebase
            .as_deref()
            .and_then(|c| self.resolver.as_ref()?.codebase_root(c))
            .is_some_and(|root| same_dir(&root, &workdir));
        if main || is_root || !workdir.join(".git").exists() {
            return Err(RpcError::new(
                code::INVALID_STATE,
                "the session works in the main checkout of the user; a revert restores only the worktree of a session",
            ));
        }
        let vcs = GitVcs::new();
        let snap =
            self.ledger(|l| revert_turn(&vcs, l, &workdir, &ctx.native, p.turn, now_secs()))?;
        Ok(RevertResult {
            session: p.session,
            turn: p.turn,
            restored: snap.n,
            snapshot: snap.commit.chars().take(12).collect(),
        })
    }
}

enum Lsp {
    Definition,
    References,
}

/// The codebase, the folder of the files, and the session of a request.
#[derive(Debug)]
pub struct Target {
    pub codebase: String,
    pub root: PathBuf,
    pub workdir: PathBuf,
    /// The native id of the session.
    pub native: Option<String>,
}

/// The cursor of a position request.
struct Cursor {
    t: Target,
    rel: String,
    line: u32,
    col: usize,
    word: (usize, usize),
    name: String,
    line_text: String,
    page: u32,
    per_page: u32,
}

impl Cursor {
    fn empty_page(&self) -> PlacePage {
        PlacePage {
            name: String::new(),
            items: Vec::new(),
            total: 0,
            page: self.page,
            per_page: self.per_page,
            truncated: false,
            source: CodeSource::Syntax,
            indexing: false,
        }
    }

    fn page_of(&self, places: Vec<Place>, source: CodeSource, indexing: bool) -> PlacePage {
        let total = places.len() as u32;
        let skip = ((self.page - 1) * self.per_page) as usize;
        PlacePage {
            name: self.name.clone(),
            items: places
                .into_iter()
                .skip(skip)
                .take(self.per_page as usize)
                .collect(),
            total,
            page: self.page,
            per_page: self.per_page,
            truncated: false,
            source,
            indexing,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers

/// A path of a request as a relative path with `/`. It must not leave the codebase.
pub fn safe_rel(path: &str) -> Res<String> {
    let norm = path.replace('\\', "/");
    let p = Path::new(&norm);
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => {
                return Err(invalid(format!(
                    "'{path}' is not a path inside the codebase"
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(invalid("the path is empty"));
    }
    Ok(parts.join("/"))
}

/// The file `rel` below `workdir`. The file must exist, and a link must not lead outside.
fn on_disk(workdir: &Path, rel: &str) -> Res<PathBuf> {
    let path = workdir.join(rel);
    let real = std::fs::canonicalize(&path).map_err(|_| RpcError::not_found("file", rel))?;
    let base = std::fs::canonicalize(workdir).map_err(server)?;
    if !real.starts_with(&base) || !real.is_file() {
        return Err(invalid(format!(
            "'{rel}' is not a file inside the codebase"
        )));
    }
    Ok(real)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn clamp_u32(v: i64) -> u32 {
    u32::try_from(v.max(0)).unwrap_or(u32::MAX)
}

fn ledger_turn(t: &aivory_vcs::Turn) -> LedgerTurn {
    LedgerTurn {
        n: t.n,
        prompt: cut(&t.prompt, api::PROMPT_CHARS),
        provider: t.provider.clone(),
        model: t.model.clone(),
        started_at: t.started_at * 1000,
        ended_at: t.ended_at * 1000,
        cost: t.cost_usd,
        tokens_in: t.usage_in.map(|v| v.max(0) as u64),
        tokens_out: t.usage_out.map(|v| v.max(0) as u64),
        snapshot: t.snapshot_ref.is_some(),
        files: t
            .files
            .iter()
            .map(|f| TurnFileStat {
                path: f.path.clone(),
                added: clamp_u32(f.added),
                removed: clamp_u32(f.removed),
            })
            .collect(),
        diagnostics_added: t.diagnostics_added.map(clamp_u32),
        diagnostics_removed: t.diagnostics_removed.map(clamp_u32),
    }
}

/// The text cut at 1 MiB on a line end.
fn cut_text(text: String) -> (String, bool) {
    if text.len() <= MAX_TEXT {
        return (text, false);
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').map_or(end, |i| i + 1);
    (text[..end].to_owned(), true)
}

fn count_lines(text: &str) -> u32 {
    u32::try_from(text.lines().count()).unwrap_or(u32::MAX)
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// The bounds of the name at a byte column of a line. A cursor just behind a name counts.
pub fn word_at(line: &str, col: usize) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let col = col.min(b.len());
    let anchor = if col < b.len() && is_ident(b[col]) {
        col
    } else if col > 0 && is_ident(b[col - 1]) {
        col - 1
    } else {
        return None;
    };
    let mut start = anchor;
    while start > 0 && is_ident(b[start - 1]) {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < b.len() && is_ident(b[end]) {
        end += 1;
    }
    // A name that starts with a digit is a number.
    if b[start].is_ascii_digit() {
        return None;
    }
    Some((start, end))
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// A byte column as a UTF-16 column.
fn utf16_col(line: &str, col: usize) -> u32 {
    let col = col.min(line.len());
    let mut end = col;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].encode_utf16().count() as u32
}

/// A UTF-16 column as a byte column.
fn byte_col_of_utf16(line: &str, col16: u32) -> u32 {
    let mut units = 0u32;
    for (i, c) in line.char_indices() {
        if units >= col16 {
            return i as u32;
        }
        units += c.len_utf16() as u32;
    }
    line.len() as u32
}

/// Lines of files, read once.
struct Files<'a> {
    root: &'a Path,
    cache: HashMap<String, Option<String>>,
}

impl<'a> Files<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            cache: HashMap::new(),
        }
    }

    fn text(&mut self, path: &str) -> Option<&str> {
        self.cache
            .entry(path.to_owned())
            .or_insert_with(|| std::fs::read_to_string(self.root.join(path)).ok())
            .as_deref()
    }

    /// The trimmed text of a line (from 1), at most 160 characters.
    fn line(&mut self, path: &str, line: u32) -> String {
        let text = self
            .text(path)
            .and_then(|t| t.lines().nth((line as usize).saturating_sub(1)))
            .unwrap_or("")
            .trim();
        clip(text)
    }

    /// Line (from 1), byte column, and name length of the start of an LSP range.
    fn start_of(&mut self, path: &str, range: &LspRange) -> (u32, u32, Option<u32>) {
        let line = range.start.line + 1;
        let text = self
            .text(path)
            .and_then(|t| t.lines().nth(range.start.line as usize))
            .unwrap_or("")
            .to_owned();
        let col = byte_col_of_utf16(&text, range.start.character);
        let len = (range.end.line == range.start.line)
            .then(|| byte_col_of_utf16(&text, range.end.character).saturating_sub(col));
        (line, col, len)
    }

    /// Line (from 1) and byte column of the end of an LSP range.
    fn end_of(&mut self, path: &str, range: &LspRange) -> (u32, u32, Option<u32>) {
        let text = self
            .text(path)
            .and_then(|t| t.lines().nth(range.end.line as usize))
            .unwrap_or("")
            .to_owned();
        (
            range.end.line + 1,
            byte_col_of_utf16(&text, range.end.character),
            None,
        )
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= 160 {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(157).collect();
    out.push_str("...");
    out
}

fn place_of_def(d: &DefHit) -> Place {
    let container = d
        .qualified
        .strip_suffix(d.name.as_str())
        .map(|q| q.trim_end_matches([':', '.', '#', '\\']).to_owned())
        .filter(|q| !q.is_empty());
    Place {
        path: d.path.clone(),
        line: d.line,
        col: d.col.saturating_sub(1),
        len: Some(d.name.len() as u32),
        preview: clip(&d.signature),
        name: Some(d.name.clone()),
        kind: Some(decl_kind(d.kind)),
        container,
    }
}

fn place_of_ref(r: &RefHit, files: &mut Files<'_>) -> Place {
    let preview = if r.text.is_empty() {
        files.line(&r.path, r.line)
    } else {
        r.text.clone()
    };
    Place {
        path: r.path.clone(),
        line: r.line,
        col: r.col.saturating_sub(1),
        len: Some(r.end_byte.saturating_sub(r.start_byte)),
        preview,
        name: None,
        kind: None,
        container: None,
    }
}

pub fn decl_kind(k: DeclKind) -> SymbolKind {
    match k {
        DeclKind::Module => SymbolKind::Module,
        DeclKind::Namespace => SymbolKind::Namespace,
        DeclKind::Class => SymbolKind::Class,
        DeclKind::Struct => SymbolKind::Struct,
        DeclKind::Enum => SymbolKind::Enum,
        DeclKind::Variant => SymbolKind::Variant,
        DeclKind::Interface => SymbolKind::Interface,
        DeclKind::Trait => SymbolKind::Trait,
        DeclKind::Impl => SymbolKind::Impl,
        DeclKind::Function => SymbolKind::Function,
        DeclKind::Method => SymbolKind::Method,
        DeclKind::Constructor => SymbolKind::Constructor,
        DeclKind::Field => SymbolKind::Field,
        DeclKind::Property => SymbolKind::Property,
        DeclKind::Variable => SymbolKind::Variable,
        DeclKind::Constant => SymbolKind::Constant,
        DeclKind::TypeAlias => SymbolKind::TypeAlias,
        DeclKind::Macro => SymbolKind::Macro,
        DeclKind::Section => SymbolKind::Section,
    }
}

pub fn kind_to_decl(k: SymbolKind) -> DeclKind {
    match k {
        SymbolKind::Module => DeclKind::Module,
        SymbolKind::Namespace => DeclKind::Namespace,
        SymbolKind::Class => DeclKind::Class,
        SymbolKind::Struct => DeclKind::Struct,
        SymbolKind::Enum => DeclKind::Enum,
        SymbolKind::Variant => DeclKind::Variant,
        SymbolKind::Interface => DeclKind::Interface,
        SymbolKind::Trait => DeclKind::Trait,
        SymbolKind::Impl => DeclKind::Impl,
        SymbolKind::Function => DeclKind::Function,
        SymbolKind::Method => DeclKind::Method,
        SymbolKind::Constructor => DeclKind::Constructor,
        SymbolKind::Field => DeclKind::Field,
        SymbolKind::Property => DeclKind::Property,
        SymbolKind::Variable => DeclKind::Variable,
        SymbolKind::Constant => DeclKind::Constant,
        SymbolKind::TypeAlias => DeclKind::TypeAlias,
        SymbolKind::Macro => DeclKind::Macro,
        SymbolKind::Section => DeclKind::Section,
    }
}

fn kind_text(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::Module => "module",
        SymbolKind::Namespace => "namespace",
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Variant => "variant",
        SymbolKind::Interface => "interface",
        SymbolKind::Trait => "trait",
        SymbolKind::Impl => "impl",
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Constructor => "constructor",
        SymbolKind::Field => "field",
        SymbolKind::Property => "property",
        SymbolKind::Variable => "variable",
        SymbolKind::Constant => "constant",
        SymbolKind::TypeAlias => "type alias",
        SymbolKind::Macro => "macro",
        SymbolKind::Section => "section",
    }
}

fn token_kind(k: aivory_syntax::TokenKind) -> TokenKind {
    use aivory_syntax::TokenKind as K;
    match k {
        K::Keyword => TokenKind::Keyword,
        K::Type => TokenKind::Type,
        K::Function => TokenKind::Function,
        K::Variable => TokenKind::Variable,
        K::String => TokenKind::String,
        K::Number => TokenKind::Number,
        K::Comment => TokenKind::Comment,
        K::Operator => TokenKind::Operator,
        K::Punctuation => TokenKind::Punctuation,
        K::Constant => TokenKind::Constant,
        K::Attribute => TokenKind::Attribute,
        K::Macro => TokenKind::Macro,
    }
}

fn outline_node(o: &aivory_syntax::OutlineItem) -> OutlineNode {
    OutlineNode {
        name: o.name.clone(),
        kind: decl_kind(o.kind),
        line: o.name_range.start_line + 1,
        start_line: o.range.start_line + 1,
        end_line: o.range.end_line + 1,
        children: o.children.iter().map(outline_node).collect(),
    }
}

fn fold(f: &aivory_syntax::FoldingRange) -> Fold {
    Fold {
        start_line: f.start_line + 1,
        end_line: f.end_line + 1,
        kind: match f.kind {
            aivory_syntax::FoldKind::Block => FoldKind::Block,
            aivory_syntax::FoldKind::Comment => FoldKind::Comment,
            aivory_syntax::FoldKind::Imports => FoldKind::Imports,
        },
    }
}

/// The spans of the lines `from..=to` as one row for each token and line.
fn highlight_rows(
    text: &str,
    spans: &[aivory_syntax::HighlightSpan],
    from: u32,
    to: u32,
) -> Vec<Highlight> {
    let mut starts: Vec<usize> = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let line_of = |byte: usize| starts.partition_point(|s| *s <= byte) - 1;
    let mut out = Vec::new();
    for s in spans {
        let (a, b) = (s.start_byte as usize, s.end_byte as usize);
        if b <= a {
            continue;
        }
        let (first, last) = (line_of(a), line_of(b - 1));
        for line in first..=last {
            let line_no = line as u32 + 1;
            if line_no < from || line_no > to {
                continue;
            }
            let begin = starts[line].max(a);
            let stop = starts.get(line + 1).copied().unwrap_or(text.len()).min(b);
            // The line break is not part of a token.
            let stop = if text[begin..stop].ends_with('\n') {
                stop - 1
            } else {
                stop
            };
            if stop > begin {
                out.push(Highlight {
                    line: line_no,
                    start: (begin - starts[line]) as u32,
                    end: (stop - starts[line]) as u32,
                    kind: token_kind(s.kind),
                });
            }
        }
    }
    out.sort_by_key(|h| (h.line, h.start));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_codebase() {
        assert_eq!(safe_rel("a/b.rs").unwrap(), "a/b.rs");
        assert_eq!(safe_rel("a\\b.rs").unwrap(), "a/b.rs");
        assert_eq!(safe_rel("./a.rs").unwrap(), "a.rs");
        assert!(safe_rel("../a.rs").is_err());
        assert!(safe_rel("a/../../b").is_err());
        assert!(safe_rel("/etc/passwd").is_err());
        assert!(safe_rel("").is_err());
        assert!(safe_rel("C:\\x").is_err() || cfg!(not(windows)));
    }

    #[test]
    fn the_name_at_a_column() {
        let line = "    self.budget.check(cb)?;";
        let (a, b) = word_at(line, 14).unwrap();
        assert_eq!(&line[a..b], "budget");
        // Just behind a name.
        let (a, b) = word_at(line, 17).unwrap();
        assert_eq!(&line[a..b], "check");
        assert!(word_at(line, 0).is_none());
        assert!(word_at("a = 12", 5).is_none());
        let (a, b) = word_at("let größe = 1;", 6).unwrap();
        assert_eq!(&"let größe = 1;"[a..b], "größe");
    }

    #[test]
    fn columns_convert_between_utf8_and_utf16() {
        let line = "a😀b";
        assert_eq!(utf16_col(line, 5), 3);
        assert_eq!(byte_col_of_utf16(line, 3), 5);
    }

    #[test]
    fn highlights_split_at_line_ends() {
        let text = "/* a\nb */ x\n";
        let spans = [aivory_syntax::HighlightSpan {
            start_byte: 0,
            end_byte: 9,
            kind: aivory_syntax::TokenKind::Comment,
        }];
        let rows = highlight_rows(text, &spans, 1, 2);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].line, rows[0].start, rows[0].end), (1, 0, 4));
        assert_eq!((rows[1].line, rows[1].start, rows[1].end), (2, 0, 4));
        // Only the lines of the viewport.
        assert_eq!(highlight_rows(text, &spans, 2, 2).len(), 1);
    }

    #[test]
    fn identifiers_are_checked() {
        assert!(valid_identifier("new_name"));
        assert!(valid_identifier("$x1"));
        assert!(!valid_identifier("1x"));
        assert!(!valid_identifier("a b"));
        assert!(!valid_identifier(""));
    }
}
