//! Agent mode: persistent JSON-RPC 2.0 server over stdin/stdout.
//!
//! Agents spawn `gradient-compiler --agent` and communicate via newline-
//! delimited JSON-RPC 2.0. The compiler holds a [`Session`](crate::query::Session)
//! in memory, eliminating re-parse overhead across queries.
//!
//! # Protocol
//!
//! One JSON object per line on stdin → one JSON object per line on stdout.
//!
//! # Example
//!
//! ```text
//! → {"jsonrpc":"2.0","id":1,"method":"load","params":{"source":"fn main() -> !{IO} ():\n    print(\"hi\")"}}
//! ← {"jsonrpc":"2.0","id":1,"result":{"ok":true,"diagnostics":[],...}}
//! → {"jsonrpc":"2.0","id":2,"method":"symbols"}
//! ← {"jsonrpc":"2.0","id":2,"result":[...]}
//! → {"jsonrpc":"2.0","id":3,"method":"shutdown"}
//! ← {"jsonrpc":"2.0","id":3,"result":{"ok":true}}
//! ```

pub mod handlers;
pub mod protocol;
pub mod server;
