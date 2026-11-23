//! Language features for Jack (`.jack`) documents.

use lsp_types::Url;

use crate::workspace::Workspace;

pub fn diagnostics(_ws: &mut Workspace, _uri: &Url) -> Vec<n2t_core::Diagnostic> {
    Vec::new()
}
