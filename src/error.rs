use thiserror::Error;

/// Typed error for every Reddit RSS failure. Never flatten into empty posts.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum FetchError {
    #[error("invalid subreddit: {0}")]
    InvalidSubreddit(String),

    #[error("empty search query")]
    InvalidQuery,

    #[error("invalid comments URL: {0}")]
    InvalidUrl(String),

    #[error("forbidden host: {0}")]
    ForbiddenHost(String),

    #[error("request blocked by Reddit (HTTP 403 or non-Atom response)")]
    Blocked,

    #[error("rate limited by Reddit (HTTP 429)")]
    RateLimited,

    #[error("HTTP {0}")]
    Http(u16),

    #[error("response is not an Atom feed")]
    NotAtom,

    #[error("request timed out")]
    Timeout,

    #[error("network error: {0}")]
    Network(String),

    #[error("failed to parse Atom feed: {0}")]
    Parse(String),
}
