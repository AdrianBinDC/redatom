//! Typed fetch failures. Never flatten these into an empty post list.

use thiserror::Error;

/// Reddit RSS client error. Tool handlers must surface the variant name.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum FetchError {
    /// Subreddit name failed sanitization.
    #[error("invalid subreddit: {0}")]
    InvalidSubreddit(String),

    /// Search query empty or over the length cap.
    #[error("empty search query")]
    InvalidQuery,

    /// Comments URL missing or unparseable.
    #[error("invalid comments URL: {0}")]
    InvalidUrl(String),

    /// Comments URL host is not on the Reddit allowlist (SSRF).
    #[error("forbidden host: {0}")]
    ForbiddenHost(String),

    /// HTTP 403, or a success status whose body is not Atom (WAF/HTML/JSON).
    #[error("request blocked by Reddit (HTTP 403 or non-Atom response)")]
    Blocked,

    /// HTTP 429, or the process-wide limiter wait budget was exhausted.
    ///
    /// `retry_after_secs` is Reddit's wait hint when known (`Retry-After` or
    /// `x-ratelimit-reset`).
    #[error("{}", rate_limited_msg(retry_after_secs))]
    RateLimited {
        /// Seconds until Reddit's quota resets, when known.
        retry_after_secs: Option<u64>,
    },

    /// Other HTTP 4xx/5xx.
    #[error("HTTP {0}")]
    Http(u16),

    /// 2xx body parsed but is not an Atom feed.
    #[error("response is not an Atom feed")]
    NotAtom,

    /// Connect or total request timeout.
    #[error("request timed out")]
    Timeout,

    /// DNS, TLS, reset, or other transport failure.
    #[error("network error: {0}")]
    Network(String),

    /// Atom well-formedness / `feed-rs` parse failure.
    #[error("failed to parse Atom feed: {0}")]
    Parse(String),
}

/// Display text for [`FetchError::RateLimited`].
fn rate_limited_msg(retry_after_secs: &Option<u64>) -> String {
    match retry_after_secs {
        Some(s) => format!("rate limited by Reddit (HTTP 429); retry after {s}s"),
        None => "rate limited by Reddit (HTTP 429)".to_string(),
    }
}
