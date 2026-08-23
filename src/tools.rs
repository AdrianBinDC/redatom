//! MCP tool schemas and handlers wrapping [`crate::client::RedatomClient`].

use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::client::RedatomClient;

/// Listing sort for the `browse_subreddit` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum BrowseSort {
    /// Hot listing (default).
    #[default]
    Hot,
    /// Newest posts.
    New,
    /// Top posts (uses `time` when set).
    Top,
    /// Rising listing.
    Rising,
    /// Controversial listing (uses `time` when set).
    Controversial,
}

/// Search ranking for the `search_reddit` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchSort {
    /// Relevance (default).
    #[default]
    Relevance,
    /// Hot ranking.
    Hot,
    /// Top ranking.
    Top,
    /// Newest first.
    New,
    /// Most comments.
    Comments,
}

/// Reddit `t=` time window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TimeRange {
    /// Past hour.
    Hour,
    /// Past day.
    Day,
    /// Past week.
    Week,
    /// Past month.
    Month,
    /// Past year.
    Year,
    /// All time.
    All,
}

/// Arguments for `browse_subreddit`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct BrowseArgs {
    /// Subreddit name without the `r/` prefix, e.g. `LocalLLaMA`.
    /// `all`, `popular`, and `+` multis are allowed.
    pub subreddit: String,
    /// Listing surface. `hot`/`rising` are the temperature surfaces.
    #[serde(default)]
    pub sort: BrowseSort,
    /// Time window. Only sent as `t=` when sort is `top` or `controversial`.
    pub time: Option<TimeRange>,
    /// Max items. Clamped to 1..=25.
    pub limit: Option<u32>,
}

/// Arguments for `search_reddit`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Search query (non-empty).
    pub query: String,
    /// Restrict to this subreddit (adds `restrict_sr=1`).
    pub subreddit: Option<String>,
    /// Ranking. Default `relevance`.
    #[serde(default)]
    pub sort: SearchSort,
    /// Time window, sent as `t=` when provided.
    pub time: Option<TimeRange>,
    /// Max results. Clamped to 1..=25.
    pub limit: Option<u32>,
}

/// Arguments for `get_post_comments`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct CommentsArgs {
    /// Post permalink or already-`.rss` URL.
    pub url: String,
    /// Max comments (submission excluded). Clamped to 1..=25.
    pub limit: Option<u32>,
}

/// Map [`TimeRange`] to Reddit `t=` query values.
fn to_str_enum_time(time: Option<TimeRange>) -> Option<&'static str> {
    time.map(|t| match t {
        TimeRange::Hour => "hour",
        TimeRange::Day => "day",
        TimeRange::Week => "week",
        TimeRange::Month => "month",
        TimeRange::Year => "year",
        TimeRange::All => "all",
    })
}

/// Map [`BrowseSort`] to listing path segments.
fn to_str_browse(sort: BrowseSort) -> &'static str {
    match sort {
        BrowseSort::Hot => "hot",
        BrowseSort::New => "new",
        BrowseSort::Top => "top",
        BrowseSort::Rising => "rising",
        BrowseSort::Controversial => "controversial",
    }
}

/// Map [`SearchSort`] to search `sort=` values.
fn to_str_search(sort: SearchSort) -> &'static str {
    match sort {
        SearchSort::Relevance => "relevance",
        SearchSort::Hot => "hot",
        SearchSort::Top => "top",
        SearchSort::New => "new",
        SearchSort::Comments => "comments",
    }
}

/// Serialize a success payload as pretty JSON tool content.
///
/// # Errors
///
/// Returns [`ErrorData`] if JSON serialization fails.
fn ok_json<T: Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))
        .map(|text| CallToolResult::success(vec![rmcp::model::ContentBlock::text(text)]))
}

/// Render a `FetchError` as `Variant: Display` for a tool error message.
pub fn tool_err_text(e: &crate::error::FetchError) -> String {
    let variant = match e {
        crate::error::FetchError::InvalidSubreddit(_) => "InvalidSubreddit",
        crate::error::FetchError::InvalidQuery => "InvalidQuery",
        crate::error::FetchError::InvalidUrl(_) => "InvalidUrl",
        crate::error::FetchError::ForbiddenHost(_) => "ForbiddenHost",
        crate::error::FetchError::Blocked => "Blocked",
        crate::error::FetchError::RateLimited { .. } => "RateLimited",
        crate::error::FetchError::Http(_) => "Http",
        crate::error::FetchError::NotAtom => "NotAtom",
        crate::error::FetchError::Timeout => "Timeout",
        crate::error::FetchError::Network(_) => "Network",
        crate::error::FetchError::Parse(_) => "Parse",
    };
    format!("{variant}: {e}")
}

/// Wrap a fetch failure as an MCP tool error result (`is_error`).
///
/// # Errors
///
/// Always returns `Ok` with an error [`CallToolResult`]; the `Result` shape is required by rmcp.
fn tool_error(e: &crate::error::FetchError) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![
        rmcp::model::ContentBlock::text(tool_err_text(e)),
    ]))
}

/// MCP server holding the shared RSS client.
pub struct ToolServer {
    /// Process-wide Reddit RSS client.
    client: Arc<RedatomClient>,
}

impl ToolServer {
    /// Wrap an existing client.
    pub fn new(client: Arc<RedatomClient>) -> Self {
        Self { client }
    }
}

#[tool_router]
impl ToolServer {
    /// Fetch a Reddit subreddit listing via public Atom/RSS.
    /// RSS only: no scores, no comment trees, ~25 items max, no API key.
    /// Use hot/rising for temperature surfaces.
    /// time only applies to top and controversial sorts.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorData`] only if success JSON cannot be serialized.
    #[tool(
        name = "browse_subreddit",
        annotations(read_only_hint = true, title = "Browse subreddit")
    )]
    async fn browse_subreddit(
        &self,
        Parameters(args): Parameters<BrowseArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .client
            .browse_subreddit(
                &args.subreddit,
                to_str_browse(args.sort),
                to_str_enum_time(args.time),
                args.limit.unwrap_or(25) as usize,
            )
            .await
        {
            Ok(result) => ok_json(&result),
            Err(e) => tool_error(&e),
        }
    }

    /// Search Reddit via public Atom/RSS.
    /// RSS only: no scores, no comment trees, ~25 items max, no API key.
    /// Set subreddit to restrict the search.
    /// time is sent as t= when provided.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorData`] only if success JSON cannot be serialized.
    #[tool(
        name = "search_reddit",
        annotations(read_only_hint = true, title = "Search Reddit")
    )]
    async fn search_reddit(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .client
            .search_reddit(
                &args.query,
                args.subreddit.as_deref(),
                to_str_search(args.sort),
                to_str_enum_time(args.time),
                args.limit.unwrap_or(25) as usize,
            )
            .await
        {
            Ok(result) => ok_json(&result),
            Err(e) => tool_error(&e),
        }
    }

    /// Fetch comments for a Reddit post via its public Atom/RSS feed.
    /// Returns a flat list (not threaded), no scores, ~25 max.
    /// Accepts a permalink or an already-.rss URL.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorData`] only if success JSON cannot be serialized.
    #[tool(
        name = "get_post_comments",
        annotations(read_only_hint = true, title = "Get post comments")
    )]
    async fn get_post_comments(
        &self,
        Parameters(args): Parameters<CommentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .client
            .get_post_comments(&args.url, args.limit.unwrap_or(25) as usize)
            .await
        {
            Ok(result) => ok_json(&result),
            Err(e) => tool_error(&e),
        }
    }
}

#[tool_handler(
    name = "reddit-rss-mcp",
    instructions = "Reddit tools are browse_subreddit, search_reddit, get_post_comments (MCP names; some clients prefix mcp__redatom__). There is no redatom skill and no search_suggestions tool. If a direct call says the tool does not exist, tool_describe that name then tool_call. Do not cargo build, curl /mcp, or read a git repo to fetch Reddit."
)]
impl ServerHandler for ToolServer {}

#[cfg(test)]
#[allow(missing_docs, clippy::missing_docs_in_private_items)]
mod tests {
    use super::*;
    use crate::client::{NoopHttp, RedatomClient};
    use crate::error::FetchError;

    #[test]
    fn server_info_names_the_three_tools() {
        let server = ToolServer::new(Arc::new(RedatomClient::new(Arc::new(NoopHttp), 0, 8)));
        let info = server.get_info();
        let text = info.instructions.expect("initialize instructions");
        assert!(text.contains("browse_subreddit"));
        assert!(text.contains("search_reddit"));
        assert!(text.contains("get_post_comments"));
        assert!(info.capabilities.tools.is_some());
        assert_eq!(info.server_info.name, "reddit-rss-mcp");
    }

    #[test]
    fn tool_err_text_includes_variant_and_display() {
        let e = FetchError::Blocked;
        let text = tool_err_text(&e);
        assert!(
            text.starts_with("Blocked:"),
            "expected variant name prefix, got {text:?}"
        );
        assert!(
            text.contains("403"),
            "expected display in message, got {text:?}"
        );
    }

    #[test]
    fn tool_err_text_rate_limited() {
        let text = tool_err_text(&FetchError::RateLimited {
            retry_after_secs: None,
        });
        assert!(text.starts_with("RateLimited:"));
    }

    #[test]
    fn tool_err_text_rate_limited_includes_retry_after() {
        let text = tool_err_text(&FetchError::RateLimited {
            retry_after_secs: Some(48),
        });
        assert!(text.starts_with("RateLimited:"));
        assert!(text.contains("retry after 48s"), "got {text:?}");
    }

    #[test]
    fn tool_err_text_forbidden_host() {
        let text = tool_err_text(&FetchError::ForbiddenHost("evil.example.com".into()));
        assert!(text.starts_with("ForbiddenHost:"));
        assert!(text.contains("evil.example.com"));
    }
}
