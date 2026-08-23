//! Reddit RSS/Atom MCP library: fetch client, tool adapters, and transports.
//!
//! Outbound Reddit access is Atom/RSS (`.rss`) only. Failures are
//! [`error::FetchError`] and must not be turned into empty post lists.

/// HTTP client, limiter, ETag cache, and URL sanitization.
pub mod client;
/// JSON-serializable post/comment result types.
pub mod domain;
/// Typed fetch errors.
pub mod error;
/// stdio and Streamable HTTP MCP transports.
pub mod mcp;
/// MCP tool schemas and handlers.
pub mod tools;
/// Sticky User-Agent selection.
pub mod ua;
