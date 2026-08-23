use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::client::RedatomClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum BrowseSort {
    #[default]
    Hot,
    New,
    Top,
    Rising,
    Controversial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchSort {
    #[default]
    Relevance,
    Hot,
    Top,
    New,
    Comments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TimeRange {
    Hour,
    Day,
    Week,
    Month,
    Year,
    All,
}

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

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
pub struct CommentsArgs {
    /// Post permalink or already-`.rss` URL.
    pub url: String,
    /// Max comments (submission excluded). Clamped to 1..=25.
    pub limit: Option<u32>,
}

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

fn to_str_browse(sort: BrowseSort) -> &'static str {
    match sort {
        BrowseSort::Hot => "hot",
        BrowseSort::New => "new",
        BrowseSort::Top => "top",
        BrowseSort::Rising => "rising",
        BrowseSort::Controversial => "controversial",
    }
}

fn to_str_search(sort: SearchSort) -> &'static str {
    match sort {
        SearchSort::Relevance => "relevance",
        SearchSort::Hot => "hot",
        SearchSort::Top => "top",
        SearchSort::New => "new",
        SearchSort::Comments => "comments",
    }
}

fn ok_json<T: Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))
        .map(|text| {
            CallToolResult::success(vec![rmcp::model::ContentBlock::text(format!(
                "```json\n{text}\n```"
            ))])
        })
}

pub struct ToolServer {
    client: Arc<RedatomClient>,
}

impl ToolServer {
    pub fn new(client: Arc<RedatomClient>) -> Self {
        Self { client }
    }
}

#[tool_router(server_handler)]
impl ToolServer {
    /// Fetch a Reddit subreddit listing via public Atom/RSS.
    /// RSS only: no scores, no comment trees, ~25 items max, no API key.
    /// Use hot/rising for temperature surfaces.
    /// time only applies to top and controversial sorts.
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
            Err(e) => Err(ErrorData::internal_error(e.to_string(), None)),
        }
    }

    /// Search Reddit via public Atom/RSS.
    /// RSS only: no scores, no comment trees, ~25 items max, no API key.
    /// Set subreddit to restrict the search.
    /// time is sent as t= when provided.
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
            Err(e) => Err(ErrorData::internal_error(e.to_string(), None)),
        }
    }

    /// Fetch comments for a Reddit post via its public Atom/RSS feed.
    /// Returns a flat list (not threaded), no scores, ~25 max.
    /// Accepts a permalink or an already-.rss URL.
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
            Err(e) => Err(ErrorData::internal_error(e.to_string(), None)),
        }
    }
}
