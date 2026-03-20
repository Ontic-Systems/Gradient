//! Gradient Language Server — entry point.
//!
//! Starts the LSP server over stdio, communicating via JSON-RPC as specified
//! by the Language Server Protocol. The server provides diagnostics, hover
//! information, and code completions for `.gr` source files.

mod backend;
mod diagnostics;

use tower_lsp::{LspService, Server};

use backend::Backend;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend::new(client));
    Server::new(stdin, stdout, socket).serve(service).await;
}
