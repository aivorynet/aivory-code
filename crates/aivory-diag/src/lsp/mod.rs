//! The LSP pool: language servers per repo lineage (`docs/design/03-code-intelligence.md`,
//! section "LSP pool"). The lens uses it for hover, definition, references, rename, semantic
//! tokens, and code actions. The pipeline uses it for the diagnostics of stage 4.

mod client;
mod pool;
mod servers;
mod uri;

pub use lsp_types;
pub use pool::{
    CodeActionItem, Ctx, FileDiagnostics, FileEdit, HoverInfo, Lineage, LspPool, MemoryProbe,
    Overlay, OverlayFile, Place, PoolConfig, SemanticToken, ServerInfo, SysinfoProbe, TickReport,
    total_memory,
};
pub use servers::{ServerSpec, known_servers, language_id};
pub use uri::{path_to_uri, relative_to, uri_to_path};
