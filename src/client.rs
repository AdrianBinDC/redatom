//! Reddit RSS fetching: browse / search / comments.
//!
//! No rmcp types here. All outbound Reddit GETs go through the shared
//! `governor` limiter + 1-in-flight semaphore; errors are typed
//! `FetchError`s and are never flattened into empty results.

use std::future::Future;
use std::num::{NonZeroU32, NonZeroUsize};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use feed_rs::model::{Entry, Feed, FeedType};
use governor::clock::Clock as _;
use governor::{DefaultDirectRateLimiter, Quota, clock::QuantaClock};
use lru::LruCache;
use reqwest::header::{
    ACCEPT, ETAG, HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
    USER_AGENT,
};
use tokio::sync::Semaphore;
use tracing::{debug, error, info, instrument};
use url::Url;

use crate::domain::{BrowseResult, Comment, CommentsResult, Post, PostSummary, SearchResult};
use crate::error::FetchError;
use crate::ua::Ua;

const COMMENTS_NOTE: &str = "RSS comments: flat list (not threaded), no scores, ~25 max.";

const ALLOWED_HOSTS: [&str; 4] = [
    "www.reddit.com",
    "reddit.com",
    "old.reddit.com",
    "np.reddit.com",
];

/// Abstraction over HTTP so tests can use a fake backend.
pub trait FeedHttp: Send + Sync + 'static {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a HeaderMap,
    ) -> BoxFuture<'a, Result<FetchResponse, Box<dyn std::error::Error + Send + Sync>>>;
}

/// Response metadata + body for one fetched URL.
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// `Retry-After` header value (seconds), when present.
    pub retry_after: Option<u64>,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Production HTTP backend (reqwest).
#[derive(Clone)]
pub struct ReqwestAdapter {
    client: reqwest::Client,
    ua: Ua,
}

impl ReqwestAdapter {
    /// Total timeout in seconds, from `REDDIT_RSS_TIMEOUT_SECS` (default 20).
    pub fn new(ua: Ua) -> Self {
        let total_timeout = std::env::var("REDDIT_RSS_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(20);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(total_timeout))
            .build()
            .expect("reqwest client builds");
        Self { client, ua }
    }

    pub fn ua(&self) -> &Ua {
        &self.ua
    }
}

impl FeedHttp for ReqwestAdapter {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a HeaderMap,
    ) -> BoxFuture<'a, Result<FetchResponse, Box<dyn std::error::Error + Send + Sync>>> {
        Box::pin(async move {
            let mut req = self.client.get(url);
            for (key, value) in headers.iter() {
                req = req.header(key, value);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let status = resp.status().as_u16();
            let headers = resp.headers();
            let content_type = headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let etag = headers
                .get(ETAG)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let last_modified = headers
                .get(LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let retry_after = headers
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok());
            let body = resp
                .bytes()
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .to_vec();
            Ok(FetchResponse {
                status,
                content_type,
                body,
                etag,
                last_modified,
                retry_after,
            })
        })
    }
}

#[derive(Clone)]
struct CacheEntry {
    etag: Option<String>,
    last_modified: Option<String>,
    feed: Feed,
}

/// One Reddit RSS client. A single instance is shared by both transports;
/// the limiter and semaphore inside are the process-wide gate.
pub struct RedatomClient {
    http: Arc<dyn FeedHttp>,
    limiter: DefaultDirectRateLimiter,
    in_flight: Arc<Semaphore>,
    /// ETag/Last-Modified + parsed feed cache, keyed by exact request URL.
    cache: Mutex<LruCache<String, CacheEntry>>,
    ua: Ua,
    limiter_wait_secs: u64,
}

impl RedatomClient {
    pub fn new(http: Arc<dyn FeedHttp>, min_interval_ms: u64, cache_cap: usize) -> Self {
        Self::with_wait(http, min_interval_ms, cache_cap, 30)
    }

    /// Same as [`RedatomClient::new`] but with an explicit max limiter wait
    /// (seconds). Exposed for tests.
    #[allow(dead_code)]
    pub fn with_wait(
        http: Arc<dyn FeedHttp>,
        min_interval_ms: u64,
        cache_cap: usize,
        limiter_wait_secs: u64,
    ) -> Self {
        let period = Duration::from_millis(min_interval_ms.max(1));
        let quota = Quota::with_period(period)
            .expect("period > 0")
            .allow_burst(NonZeroU32::MIN);
        let cap = NonZeroUsize::try_from(cache_cap.max(1))
            .unwrap_or_else(|_| NonZeroUsize::new(256).expect("256 > 0"));
        Self {
            http,
            limiter: DefaultDirectRateLimiter::new(quota, Default::default(), Default::default()),
            in_flight: Arc::new(Semaphore::new(1)),
            cache: Mutex::new(LruCache::new(cap)),
            ua: Ua::from_env(),
            limiter_wait_secs,
        }
    }

    async fn acquire_permit(&self, wait_secs: u64) -> Result<(), FetchError> {
        let start = std::time::Instant::now();
        let deadline = start + Duration::from_secs(wait_secs);
        loop {
            match self.limiter.check() {
                Ok(_) => {
                    let wait_ms = start.elapsed().as_millis();
                    if wait_ms > 0 {
                        debug!("limiter wait: {wait_ms} ms");
                    }
                    return Ok(());
                }
                Err(next) => {
                    if start.elapsed() >= Duration::from_secs(wait_secs) {
                        error!("no limiter permit within {wait_secs}s; giving up");
                        return Err(FetchError::RateLimited);
                    }
                    let clock = QuantaClock::default();
                    let wait = next.wait_time_from(clock.now()).min(deadline - start);
                    tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
                }
            }
        }
    }

    fn make_headers(&self, url: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/atom+xml, application/xml;q=0.9, text/xml;q=0.8"),
        );
        let ua = HeaderValue::from_str(&self.ua.header()).expect("UA from ua.rs is a valid header");
        headers.insert(USER_AGENT, ua);

        if let Ok(mut cache) = self.cache.lock()
            && let Some(entry) = cache.get(url)
        {
            if let Some(etag) = &entry.etag
                && let Ok(v) = HeaderValue::from_str(etag)
            {
                headers.insert(IF_NONE_MATCH, v);
            }
            if let Some(lm) = &entry.last_modified
                && let Ok(v) = HeaderValue::from_str(lm)
            {
                headers.insert(IF_MODIFIED_SINCE, v);
            }
        }
        headers
    }

    /// Parse + cache an already-received fresh (non-304) response body.
    fn parse_and_cache(&self, url: &str, resp: FetchResponse) -> Result<Feed, FetchError> {
        let looks_like_xml = resp.content_type.to_ascii_lowercase().contains("xml")
            || resp.body.starts_with(b"<?xml")
            || (resp.body.len() >= 5 && &resp.body[0..5] == b"<?xml");

        if !looks_like_xml {
            error!(
                content_type = %resp.content_type,
                "success-status response is not XML (WAF/HTML/JSON)"
            );
            return Err(FetchError::Blocked);
        }

        let feed = feed_rs::parser::parse(&resp.body[..]).map_err(|e| {
            error!(%e, "Atom parse failed");
            FetchError::Parse(e.to_string())
        })?;

        if feed.feed_type != FeedType::Atom {
            error!(feed_type = ?feed.feed_type, "not an Atom feed");
            return Err(FetchError::NotAtom);
        }

        {
            let mut cache = self.cache.lock().expect("cache lock not poisoned");
            cache.put(
                url.to_string(),
                CacheEntry {
                    etag: resp.etag,
                    last_modified: resp.last_modified,
                    feed: feed.clone(),
                },
            );
        }
        info!(url, items = feed.entries.len(), "fetched feed");
        Ok(feed)
    }

    /// GET one feed URL with the shared limiter + 1-in-flight semaphore.
    /// Returns the parsed Atom feed (cache hit or fresh parse).
    #[instrument(skip(self, url), fields(url))]
    async fn fetch_feed(&self, url: &str) -> Result<Feed, FetchError> {
        self.acquire_permit(self.limiter_wait_secs).await?;

        let _in_flight = self
            .in_flight
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore not closed");

        let headers = self.make_headers(url);
        let resp = self.http.get(url, &headers).await.map_err(|e| {
            error!(%e, "network error");
            FetchError::Network(e.to_string())
        })?;

        let status = resp.status;

        match status {
            429 => {
                error!("HTTP 429 from reddit");
                return Err(FetchError::RateLimited);
            }
            403 => {
                error!("HTTP 403 (WAF) from reddit");
                return Err(FetchError::Blocked);
            }
            s if !(200..300).contains(&s) => {
                error!(status = s, "HTTP error");
                return Err(FetchError::Http(s));
            }
            304 => {
                let cached = self.cache.lock().ok().and_then(|mut c| c.get(url).cloned());
                match cached {
                    Some(entry) => {
                        info!(url, "304 cache hit");
                        Ok(entry.feed)
                    }
                    None => {
                        error!("304 with empty cache (evicted)");
                        Err(FetchError::NotAtom)
                    }
                }
            }
            _ => self.parse_and_cache(url, resp),
        }
    }

    /// Browse a subreddit listing surface.
    pub async fn browse_subreddit(
        &self,
        subreddit: &str,
        sort: &str,
        time: Option<&str>,
        limit: usize,
    ) -> Result<BrowseResult, FetchError> {
        let sub = sanitize_subreddit(subreddit)?;
        let limit = limit.clamp(1, 25);
        let sort = if sort.is_empty() { "hot" } else { sort };
        let need_time = matches!(sort, "top" | "controversial");

        let url = build_browse_url(&sub, sort, time.filter(|_| need_time), limit);
        let feed = self.fetch_feed(&url).await?;

        let posts: Vec<Post> = feed.entries.iter().take(limit).map(entry_to_post).collect();
        Ok(BrowseResult {
            source: url,
            count: posts.len(),
            posts,
        })
    }

    /// Search Reddit (sitewide or within a subreddit).
    pub async fn search_reddit(
        &self,
        query: &str,
        subreddit: Option<&str>,
        sort: &str,
        time: Option<&str>,
        limit: usize,
    ) -> Result<SearchResult, FetchError> {
        let query = query.trim();
        if query.is_empty() || query.len() > 512 {
            return Err(FetchError::InvalidQuery);
        }
        let sub = match subreddit {
            Some(s) if !s.trim().is_empty() => Some(sanitize_subreddit(s)?),
            _ => None,
        };
        let sort = if sort.is_empty() { "relevance" } else { sort };
        let limit = limit.clamp(1, 25);

        let url = build_search_url(query, sub.as_deref(), sort, time, limit);
        let feed = self.fetch_feed(&url).await?;

        let posts: Vec<Post> = feed.entries.iter().take(limit).map(entry_to_post).collect();
        Ok(SearchResult {
            source: url,
            query: query.to_string(),
            count: posts.len(),
            posts,
        })
    }

    /// Fetch a post's comments feed (shallow, flat).
    pub async fn get_post_comments(
        &self,
        url: &str,
        limit: usize,
    ) -> Result<CommentsResult, FetchError> {
        let normalized = normalize_comments_url(url)?;
        let parsed =
            Url::parse(&normalized).map_err(|_| FetchError::InvalidUrl(url.to_string()))?;
        let host = parsed.host_str().unwrap_or_default().to_string();
        if !ALLOWED_HOSTS.contains(&host.as_str()) {
            return Err(FetchError::ForbiddenHost(host));
        }

        let limit = limit.clamp(1, 25);
        let feed = self.fetch_feed(&normalized).await?;

        let (post, comments) = split_post_and_comments(&feed, limit);
        Ok(CommentsResult {
            source: normalized,
            post,
            count: comments.len(),
            comments,
            note: COMMENTS_NOTE.to_string(),
        })
    }
}

/// Strip `r/`/leading slash; allow only `[A-Za-z0-9_+]`.
pub fn sanitize_subreddit(input: &str) -> Result<String, FetchError> {
    let cleaned = input
        .trim()
        .trim_start_matches('/')
        .trim_start_matches("r/");
    if cleaned.is_empty() || cleaned.len() > 200 {
        return Err(FetchError::InvalidSubreddit(input.to_string()));
    }
    for c in cleaned.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '+') {
            return Err(FetchError::InvalidSubreddit(input.to_string()));
        }
    }
    Ok(cleaned.to_string())
}

fn build_browse_url(sub: &str, sort: &str, time: Option<&str>, limit: usize) -> String {
    let mut url = Url::parse(&format!("https://www.reddit.com/r/{sub}/{sort}.rss"))
        .expect("static URL template is valid");
    url.query_pairs_mut()
        .append_pair("limit", &limit.to_string());
    if let Some(t) = time {
        url.query_pairs_mut().append_pair("t", t);
    }
    url.into()
}

fn build_search_url(
    query: &str,
    sub: Option<&str>,
    sort: &str,
    time: Option<&str>,
    limit: usize,
) -> String {
    let base = match sub {
        Some(s) => format!("https://www.reddit.com/r/{s}/search.rss"),
        None => "https://www.reddit.com/search.rss".to_string(),
    };
    let mut url = Url::parse(&base).expect("static URL template is valid");
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("sort", sort)
        .append_pair("limit", &limit.to_string());
    if let Some(t) = time {
        url.query_pairs_mut().append_pair("t", t);
    }
    if sub.is_some() {
        url.query_pairs_mut().append_pair("restrict_sr", "1");
    }
    url.into()
}

/// Normalize a comments permalink: add scheme, drop fragment/query,
/// append `.rss` unless already present.
pub fn normalize_comments_url(input: &str) -> Result<String, FetchError> {
    let s = input.trim();
    if s.is_empty() || s.len() > 2048 {
        return Err(FetchError::InvalidUrl(input.to_string()));
    }
    let with_scheme = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else if let Some(rest) = s.strip_prefix("www.reddit.com/") {
        format!("https://www.reddit.com/{rest}")
    } else if let Some(rest) = s.strip_prefix("reddit.com/") {
        format!("https://www.reddit.com/{rest}")
    } else {
        format!("https://www.reddit.com/{s}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|e| FetchError::InvalidUrl(format!("{e}")))?;
    url.set_fragment(None);
    url.query_pairs_mut().clear();
    let path = url.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        return Err(FetchError::InvalidUrl(input.to_string()));
    }
    let suffix = if path.ends_with(".rss") { "" } else { ".rss" };
    Ok(format!(
        "{}://{host}{path}{suffix}",
        url.scheme(),
        host = url.host_str().unwrap_or_default()
    ))
}

fn entry_to_post(entry: &Entry) -> Post {
    Post {
        title: entry
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_default(),
        author: entry
            .authors
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        link: entry
            .links
            .first()
            .map(|l| l.href.clone())
            .unwrap_or_default(),
        published: entry
            .published
            .or(entry.updated)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        id: entry.id.clone(),
        subreddit: entry
            .categories
            .first()
            .map(|c| c.term.clone())
            .unwrap_or_default(),
        excerpt: entry
            .content
            .as_ref()
            .and_then(|c| c.body.clone())
            .map(|body| html_to_excerpt(&body))
            .unwrap_or_default(),
    }
}

/// Crude HTML -> plain text: drop tags, decode common entities, squeeze
/// whitespace, cap at 500 chars. Good enough for excerpts; no HTML parser
/// on the hot path.
fn html_to_excerpt(html: &str) -> String {
    let mut out = String::with_capacity(512);
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            _c if in_tag => {}
            ' ' | '\t' | '\n' | '\r' => {
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    let s = out.trim().to_string();
    let s = s.replace("&amp;", "&");
    let s = s.replace("&#39;", "'");
    let s = s.replace("&quot;", "\"");
    let s = s.replace("&lt;", "<");
    let s = s.replace("&gt;", ">");
    s.chars().take(500).collect()
}

fn split_post_and_comments(feed: &Feed, limit: usize) -> (Option<PostSummary>, Vec<Comment>) {
    let entries = &feed.entries;
    if entries.is_empty() {
        return (None, Vec::new());
    }
    // The submission is the entry whose permalink has no trailing comment id,
    // i.e. it matches /comments/{id}/ but not /comments/{id}/{comment_id}/.
    let post_idx = entries.iter().position(is_submission_entry).unwrap_or(0);
    let post = Some(PostSummary {
        title: entries[post_idx]
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_default(),
        author: entries[post_idx]
            .authors
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        link: entries[post_idx]
            .links
            .first()
            .map(|l| l.href.clone())
            .unwrap_or_default(),
        published: entries[post_idx]
            .published
            .or(entries[post_idx].updated)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
    });

    let comments: Vec<Comment> = entries
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != post_idx)
        .take(limit)
        .map(|(_, e)| Comment {
            author: e
                .authors
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default(),
            text: e
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .map(|body| html_to_excerpt(&body))
                .unwrap_or_default(),
            link: e.links.first().map(|l| l.href.clone()).unwrap_or_default(),
            published: e
                .published
                .or(e.updated)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default(),
        })
        .collect();
    (post, comments)
}

/// A submission permalink looks like .../comments/{post_id}/ (no extra path
/// segment); comments have one more segment .../comments/{post}/{comment}/.
fn is_submission_entry(entry: &Entry) -> bool {
    if entry.id.starts_with("t3_") {
        return true;
    }
    entry.links.iter().any(|l| {
        let mut segments = l.href.rsplit('/').filter(|s| !s.is_empty());
        let last = segments.next();
        let second_last = segments.next();
        let third = segments.next();
        last.is_some()
            && second_last.is_some()
            && third == Some("comments")
            && segments.next() != Some("comments")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_r_prefix() {
        assert_eq!(sanitize_subreddit("r/Foo").unwrap(), "Foo");
        assert_eq!(sanitize_subreddit("/r/Foo").unwrap(), "Foo");
        assert_eq!(
            sanitize_subreddit("LocalLLaMA+selfhosted").unwrap(),
            "LocalLLaMA+selfhosted"
        );
        assert!(sanitize_subreddit("../etc").is_err());
        assert!(sanitize_subreddit("a/b").is_err());
        assert!(sanitize_subreddit("").is_err());
        assert!(sanitize_subreddit("r/").is_err());
    }

    #[test]
    fn normalize_comments_url_shapes() {
        assert_eq!(
            normalize_comments_url("https://www.reddit.com/r/x/comments/abc/t/").unwrap(),
            "https://www.reddit.com/r/x/comments/abc/t.rss"
        );
        assert_eq!(
            normalize_comments_url("reddit.com/r/x/comments/abc/t/").unwrap(),
            "https://www.reddit.com/r/x/comments/abc/t.rss"
        );
        assert_eq!(
            normalize_comments_url("https://old.reddit.com/r/x/comments/abc/t.rss").unwrap(),
            "https://old.reddit.com/r/x/comments/abc/t.rss"
        );
        assert_eq!(
            normalize_comments_url("https://www.reddit.com/r/x/comments/abc/t/?ctx=1#top").unwrap(),
            "https://www.reddit.com/r/x/comments/abc/t.rss"
        );
        assert!(normalize_comments_url("").is_err());
    }

    #[test]
    fn excerpt_strips_tags_and_entities() {
        assert_eq!(
            html_to_excerpt("<div class=\"md\"><p>Hi &amp; hello</p></div>"),
            "Hi & hello"
        );
    }

    #[test]
    fn submission_vs_comment_detection() {
        let feed =
            feed_rs::parser::parse(include_str!("../tests/fixtures/comments.atom").as_bytes())
                .expect("fixture parses");
        let (post, comments) = split_post_and_comments(&feed, 25);
        let post = post.expect("post present");
        assert_eq!(post.title, "Hot post about LLMs");
        assert_eq!(comments.len(), 2);
        assert!(post.link.contains("/comments/1vv0000/"));
        assert!(!post.link.contains("/1vv0000/1"));
    }
}
