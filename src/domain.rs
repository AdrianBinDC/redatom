use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Post {
    pub title: String,
    pub author: String,
    pub link: String,
    pub published: String,
    pub id: String,
    pub subreddit: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostSummary {
    pub title: String,
    pub author: String,
    pub link: String,
    pub published: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comment {
    pub author: String,
    pub text: String,
    pub link: String,
    pub published: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowseResult {
    pub source: String,
    pub count: usize,
    pub posts: Vec<Post>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    pub source: String,
    pub query: String,
    pub count: usize,
    pub posts: Vec<Post>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommentsResult {
    pub source: String,
    pub post: Option<PostSummary>,
    pub count: usize,
    pub comments: Vec<Comment>,
    pub note: String,
}
