//! JSON-serializable feed results returned to MCP tool callers.

use serde::Serialize;

/// One listing or search hit parsed from Atom.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Post {
    /// Entry title.
    pub title: String,
    /// Author name from Atom (often `/u/…`).
    pub author: String,
    /// Absolute permalink.
    pub link: String,
    /// RFC3339 timestamp when present, otherwise the raw Atom value.
    pub published: String,
    /// Atom entry id.
    pub id: String,
    /// Category/subreddit label; may be empty.
    pub subreddit: String,
    /// Plain-text excerpt, max 500 characters.
    pub excerpt: String,
}

/// Submission header on a comments feed (no excerpt).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostSummary {
    /// Submission title.
    pub title: String,
    /// Submission author.
    pub author: String,
    /// Submission permalink.
    pub link: String,
    /// RFC3339 timestamp when present.
    pub published: String,
}

/// One flat comment from a post `.rss` feed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comment {
    /// Comment author.
    pub author: String,
    /// Plain-text body, max 500 characters.
    pub text: String,
    /// Comment permalink when present.
    pub link: String,
    /// RFC3339 timestamp when present.
    pub published: String,
}

/// Result of [`crate::client::RedatomClient::browse_subreddit`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowseResult {
    /// Exact URL fetched after sanitization.
    pub source: String,
    /// Number of posts in [`Self::posts`].
    pub count: usize,
    /// Listing entries (≤25).
    pub posts: Vec<Post>,
}

/// Result of [`crate::client::RedatomClient::search_reddit`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    /// Exact URL fetched.
    pub source: String,
    /// Search query after trim.
    pub query: String,
    /// Number of posts in [`Self::posts`].
    pub count: usize,
    /// Search hits (≤25).
    pub posts: Vec<Post>,
}

/// Result of [`crate::client::RedatomClient::get_post_comments`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommentsResult {
    /// Exact comments `.rss` URL fetched.
    pub source: String,
    /// Submission entry when detected.
    pub post: Option<PostSummary>,
    /// Number of comments in [`Self::comments`].
    pub count: usize,
    /// Flat comment list (not threaded).
    pub comments: Vec<Comment>,
    /// RSS limitations blurb for the model.
    pub note: String,
}
