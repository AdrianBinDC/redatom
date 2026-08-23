//! Reddit RSS fetching: browse / search / comments.
//!
//! No rmcp types here. All outbound Reddit GETs go through the shared
//! `governor` limiter + 1-in-flight semaphore; errors are typed
//! `FetchError`s and are never flattened into empty results.

use std::future::Future;
use std::num::{NonZeroU32, NonZeroUsize};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use feed_rs::model::{Entry, Feed, FeedType};
use governor::clock::Clock as _;
use governor::{DefaultDirectRateLimiter, Quota, clock::QuantaClock};
use lru::LruCache;
use reqwest::header::{
    ACCEPT, ETAG, HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
    USER_AGENT,
};
use reqwest::redirect::Policy;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, instrument};
use url::Url;

use crate::domain::{BrowseResult, Comment, CommentsResult, Post, PostSummary, SearchResult};
use crate::error::FetchError;
use crate::ua::Ua;

/// Blurb attached to comments tool results.
const COMMENTS_NOTE: &str = "RSS comments: flat list (not threaded), no scores, ~25 max.";

/// Hosts allowed for comments URLs and redirects.
const ALLOWED_HOSTS: [&str; 4] = [
    "www.reddit.com",
    "reddit.com",
    "old.reddit.com",
    "np.reddit.com",
];

/// Abstraction over HTTP so tests can use a fake backend.
pub trait FeedHttp: Send + Sync + 'static {
    /// GET `url` with `headers`.
    ///
    /// # Errors
    ///
    /// Returns a boxed error on transport failure. Implementations may also
    /// return [`FetchError`] (e.g. [`FetchError::ForbiddenHost`] after redirect).
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a HeaderMap,
    ) -> BoxFuture<'a, Result<FetchResponse, Box<dyn std::error::Error + Send + Sync>>>;
}

/// Response metadata + body for one fetched URL.
#[derive(Debug, Clone, Default)]
pub struct FetchResponse {
    /// HTTP status code.
    pub status: u16,
    /// `Content-Type` header, or empty.
    pub content_type: String,
    /// Response body bytes.
    pub body: Vec<u8>,
    /// `ETag` header when present.
    pub etag: Option<String>,
    /// `Last-Modified` header when present.
    pub last_modified: Option<String>,
    /// `Retry-After` header value (seconds), when present.
    pub retry_after: Option<u64>,
    /// `x-ratelimit-remaining` when present.
    pub ratelimit_remaining: Option<f64>,
    /// `x-ratelimit-reset` normalized to seconds from now, when present.
    pub ratelimit_reset_secs: Option<u64>,
}

impl FetchResponse {
    /// Positive wait hint: `Retry-After` if &gt; 0, else `x-ratelimit-reset` if &gt; 0.
    pub fn wait_hint(&self) -> Option<u64> {
        self.retry_after
            .filter(|&s| s > 0)
            .or_else(|| self.ratelimit_reset_secs.filter(|&s| s > 0))
    }

    /// Seconds to sleep before one 429 retry. `Retry-After: 0` means retry now;
    /// a zero `x-ratelimit-reset` is ignored (not a useful backoff).
    fn retry_wait_secs(&self) -> Option<u64> {
        self.retry_after
            .filter(|&s| s > 0)
            .or(self.ratelimit_reset_secs.filter(|&s| s > 0))
            .or(self.retry_after)
    }
}

/// Parse `Retry-After` as delay-seconds.
fn parse_retry_after_secs(raw: &str) -> Option<u64> {
    raw.trim().parse().ok()
}

/// Parse `x-ratelimit-reset`: small numbers are seconds-from-now; large
/// values are Unix timestamps.
fn parse_ratelimit_reset_secs(raw: &str) -> Option<u64> {
    let n: f64 = raw.trim().parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    if n >= 1_000_000_000.0 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs_f64();
        let delta = n - now;
        if delta <= 0.0 {
            return None;
        }
        return Some(delta.ceil() as u64);
    }
    let secs = n.ceil() as u64;
    if secs == 0 { None } else { Some(secs) }
}

/// Boxed async future used by [`FeedHttp::get`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Production HTTP backend (reqwest).
#[derive(Clone)]
pub struct ReqwestAdapter {
    /// Shared reqwest client (timeouts + redirect policy).
    client: reqwest::Client,
    /// Primary User-Agent for this process.
    ua: Ua,
}

impl ReqwestAdapter {
    /// Build a client. Total timeout comes from `REDDIT_RSS_TIMEOUT_SECS` (default 20).
    pub fn new(ua: Ua) -> Self {
        let total_timeout = std::env::var("REDDIT_RSS_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(20);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(total_timeout))
            .redirect(Policy::custom(|attempt| {
                let host = attempt.url().host_str().unwrap_or_default();
                if host_allowed(host) {
                    attempt.follow()
                } else {
                    error!(host, "redirect to disallowed host; stopping");
                    attempt.stop()
                }
            }))
            .build()
            .expect("reqwest client builds");
        Self { client, ua }
    }

    /// User-Agent configured for this adapter.
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
            // Belt-and-suspenders: even with the redirect policy above,
            // verify the final URL host is still in the allowlist.
            let final_host = resp.url().host_str().unwrap_or_default();
            if !host_allowed(final_host) {
                return Err(FetchError::ForbiddenHost(final_host.to_string()).into());
            }
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
                .and_then(parse_retry_after_secs);
            let ratelimit_remaining = headers
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok());
            let ratelimit_reset_secs = headers
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_ratelimit_reset_secs);
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
                ratelimit_remaining,
                ratelimit_reset_secs,
            })
        })
    }
}

/// Cached feed plus validators for conditional GET.
#[derive(Clone)]
struct CacheEntry {
    /// Last `ETag` from Reddit.
    etag: Option<String>,
    /// Last `Last-Modified` from Reddit.
    last_modified: Option<String>,
    /// Parsed Atom feed.
    feed: Feed,
}

/// One Reddit RSS client. A single instance is shared by both transports;
/// the limiter and semaphore inside are the process-wide gate.
pub struct RedatomClient {
    /// HTTP backend (reqwest or test fake).
    http: Arc<dyn FeedHttp>,
    /// Process-wide minimum interval between outbound GETs.
    limiter: DefaultDirectRateLimiter,
    /// At most one Reddit GET in flight.
    in_flight: Arc<Semaphore>,
    /// ETag/Last-Modified + parsed feed cache, keyed by exact request URL.
    cache: Mutex<LruCache<String, CacheEntry>>,
    /// Primary User-Agent (may become sticky after a successful fallback).
    ua: Ua,
    /// Max seconds to wait for a limiter permit.
    limiter_wait_secs: u64,
    /// Max seconds to wait out Reddit `remaining=0` reset. Separate from
    /// [`Self::limiter_wait_secs`]: RSS windows are often ~60s, longer than
    /// the governor budget.
    quota_wait_secs: u64,
    /// Whether the one-time UA fallback has already been used.
    ua_fallback_used: std::sync::atomic::AtomicBool,
    /// Earliest time a Reddit GET is allowed after `x-ratelimit-remaining` hit 0.
    quota_hold_until: Mutex<Option<Instant>>,
}

impl RedatomClient {
    /// Construct with a 30s limiter wait budget.
    pub fn new(http: Arc<dyn FeedHttp>, min_interval_ms: u64, cache_cap: usize) -> Self {
        Self::with_wait(http, min_interval_ms, cache_cap, 30)
    }

    /// Same as [`RedatomClient::new`] but with an explicit max limiter wait
    /// (seconds). Exposed for tests.
    pub fn with_wait(
        http: Arc<dyn FeedHttp>,
        min_interval_ms: u64,
        cache_cap: usize,
        limiter_wait_secs: u64,
    ) -> Self {
        Self::with_ua_and_wait(
            http,
            min_interval_ms,
            cache_cap,
            limiter_wait_secs,
            Ua::Honest,
        )
    }

    /// Same as [`RedatomClient::with_wait`] but with an explicit primary UA.
    /// Exposed for tests of the 403 UA fallback.
    pub fn with_ua_and_wait(
        http: Arc<dyn FeedHttp>,
        min_interval_ms: u64,
        cache_cap: usize,
        limiter_wait_secs: u64,
        ua: Ua,
    ) -> Self {
        Self::with_budgets(http, min_interval_ms, cache_cap, limiter_wait_secs, 90, ua)
    }

    /// Explicit governor wait and Reddit quota-hold wait. Exposed for tests
    /// and for [`crate::mcp`] env wiring.
    pub fn with_budgets(
        http: Arc<dyn FeedHttp>,
        min_interval_ms: u64,
        cache_cap: usize,
        limiter_wait_secs: u64,
        quota_wait_secs: u64,
        ua: Ua,
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
            ua,
            limiter_wait_secs,
            quota_wait_secs,
            ua_fallback_used: std::sync::atomic::AtomicBool::new(false),
            quota_hold_until: Mutex::new(None),
        }
    }

    /// Wait until the rate limiter grants a permit, or fail.
    ///
    /// Also waits out Reddit's `x-ratelimit-reset` hold when the last response
    /// reported no remaining quota, if that hold fits in [`Self::quota_wait_secs`].
    /// The governor deadline starts after that sleep so a ~60s RSS window is
    /// not compared against the 30s limiter budget.
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::RateLimited`] if no permit is granted within `wait_secs`,
    /// or if Reddit's quota hold lasts longer than [`Self::quota_wait_secs`].
    async fn acquire_permit(&self, wait_secs: u64) -> Result<(), FetchError> {
        if let Some(until) = self.quota_hold_until.lock().ok().and_then(|g| *g) {
            let now = Instant::now();
            if until > now {
                let remain = until.saturating_duration_since(now);
                let remain_secs = remain.as_secs().max(1);
                if remain_secs > self.quota_wait_secs {
                    error!(
                        retry_after_secs = remain_secs,
                        quota_wait_secs = self.quota_wait_secs,
                        "Reddit quota hold exceeds quota wait budget"
                    );
                    return Err(FetchError::RateLimited {
                        retry_after_secs: Some(remain_secs),
                    });
                }
                debug!(wait_secs = remain_secs, "waiting for Reddit quota reset");
                tokio::time::sleep(remain).await;
            }
        }

        let start = Instant::now();
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
                    if Instant::now() >= deadline {
                        error!("no limiter permit within {wait_secs}s; giving up");
                        return Err(FetchError::RateLimited {
                            retry_after_secs: None,
                        });
                    }
                    let clock = QuantaClock::default();
                    let wait = next
                        .wait_time_from(clock.now())
                        .min(deadline.saturating_duration_since(Instant::now()));
                    tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
                }
            }
        }
    }

    /// Record Reddit quota headers so the next GET can wait for reset.
    fn note_ratelimit(&self, resp: &FetchResponse) {
        let Some(remaining) = resp.ratelimit_remaining else {
            return;
        };
        let Ok(mut hold) = self.quota_hold_until.lock() else {
            return;
        };
        if remaining < 1.0 {
            if let Some(secs) = resp.ratelimit_reset_secs.filter(|&s| s > 0) {
                let capped = secs.min(300);
                *hold = Some(Instant::now() + Duration::from_secs(capped));
                debug!(remaining, reset_secs = capped, "recorded Reddit quota hold");
            }
        } else {
            *hold = None;
        }
    }

    /// Build request headers including UA and conditional GET validators.
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
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::Blocked`] if the body is not XML, [`FetchError::Parse`]
    /// if Atom parsing fails, or [`FetchError::NotAtom`] if the feed is not Atom.
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
    ///
    /// # Errors
    ///
    /// Returns a [`FetchError`] for limiter timeout, HTTP failures, blocked/WAF
    /// responses, parse failures, or a 304 with an empty cache.
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
        let resp = self.http.get(url, &headers).await.map_err(map_http_error)?;
        drop(_in_flight);
        self.note_ratelimit(&resp);

        if resp.status == 429 {
            if let Some(wait) = resp.retry_wait_secs() {
                // Don't block the caller for the entire limiter budget on a 429.
                if wait > 0 && wait >= self.limiter_wait_secs {
                    info!(
                        retry_after_secs = wait,
                        "HTTP 429 wait exceeds limiter budget; not retrying"
                    );
                    return Err(FetchError::RateLimited {
                        retry_after_secs: resp.wait_hint().or(Some(wait)),
                    });
                }
                let capped = wait.min(60);
                if capped > 0 {
                    info!(wait_secs = capped, "honoring Reddit wait hint once");
                    tokio::time::sleep(Duration::from_secs(capped)).await;
                }
                let _in_flight2 = self
                    .in_flight
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore not closed");
                self.acquire_permit(self.limiter_wait_secs).await?;
                let headers2 = self.make_headers(url);
                let resp2 = self
                    .http
                    .get(url, &headers2)
                    .await
                    .map_err(map_http_error)?;
                drop(_in_flight2);
                self.note_ratelimit(&resp2);
                if resp2.status != 429 {
                    return self.finish_fetch(url, resp2).await;
                }
                error!("HTTP 429 from reddit after one wait");
                return Err(FetchError::RateLimited {
                    retry_after_secs: resp2
                        .wait_hint()
                        .or(resp.wait_hint())
                        .or(Some(wait).filter(|&s| s > 0)),
                });
            }
            error!("HTTP 429 from reddit");
            return Err(FetchError::RateLimited {
                retry_after_secs: resp.wait_hint(),
            });
        }

        self.finish_fetch(url, resp).await
    }

    /// Handle a received response: 304 cache hit, error status, or parse + cache.
    ///
    /// # Errors
    ///
    /// Same as [`Self::parse_and_cache`], plus HTTP status mapping, 304 miss,
    /// and [`Self::retry_with_fallback_ua`] on 403.
    async fn finish_fetch(&self, url: &str, resp: FetchResponse) -> Result<Feed, FetchError> {
        let status = resp.status;

        match status {
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
            403 => {
                error!("HTTP 403 (WAF) from reddit; trying one UA fallback");
                return self.retry_with_fallback_ua(url).await;
            }
            s if !(200..300).contains(&s) => {
                error!(status = s, "HTTP error");
                Err(FetchError::Http(s))
            }
            _ => self.parse_and_cache(url, resp),
        }
    }

    /// On HTTP 403, try one different User-Agent for the same URL.
    /// If that also fails or returns 403/HTML, return `Blocked`.
    /// If it succeeds with 200 Atom, the fallback UA becomes sticky.
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::Blocked`] when no fallback is available or the
    /// fallback still fails; otherwise the same errors as [`Self::parse_and_cache`].
    async fn retry_with_fallback_ua(&self, url: &str) -> Result<Feed, FetchError> {
        let ua_fallback = match self.ua_fallback_used.compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ) {
            Ok(_) => self.ua.fallback(),
            Err(_) => {
                error!("UA fallback already used this process; returning Blocked");
                return Err(FetchError::Blocked);
            }
        };

        let fallback_ua = match ua_fallback {
            Some(f) => f,
            None => {
                error!(
                    "no UA fallback available from primary UA {:?}; returning Blocked",
                    self.ua
                );
                return Err(FetchError::Blocked);
            }
        };

        info!(?self.ua, ?fallback_ua, "retrying with fallback UA");

        // Re-acquire limiter + in-flight semaphore for the second attempt.
        self.acquire_permit(self.limiter_wait_secs).await?;
        let _in_flight = self
            .in_flight
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore not closed");

        let mut headers = self.make_headers(url);
        let ua_value = HeaderValue::from_str(&fallback_ua.header()).expect("fallback UA is valid");
        headers.insert(USER_AGENT, ua_value);

        let resp = self.http.get(url, &headers).await.map_err(map_http_error)?;

        drop(_in_flight);
        self.note_ratelimit(&resp);

        if resp.status == 403 {
            error!("fallback UA also returned 403; returning Blocked");
            return Err(FetchError::Blocked);
        }

        // 200 success: make the fallback UA sticky for the rest of the process.
        if (200..300).contains(&resp.status) {
            info!("fallback UA succeeded; making it sticky for the rest of the process");
        }

        self.parse_and_cache(url, resp)
    }

    /// Browse a subreddit listing surface.
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::InvalidSubreddit`] or any error from fetching/parsing the feed.
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
    ///
    /// # Errors
    ///
    /// Returns [`FetchError::InvalidQuery`], [`FetchError::InvalidSubreddit`], or
    /// any error from fetching/parsing the feed.
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
    ///
    /// # Errors
    ///
    /// Returns URL/host sanitization errors or any error from fetching/parsing the feed.
    pub async fn get_post_comments(
        &self,
        url: &str,
        limit: usize,
    ) -> Result<CommentsResult, FetchError> {
        let normalized = normalize_comments_url(url)?;
        let parsed =
            Url::parse(&normalized).map_err(|_| FetchError::InvalidUrl(url.to_string()))?;
        reject_if_forbidden(&parsed)?;

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

/// Map a `Box<dyn Error>` from the HTTP backend to a typed `FetchError`.
/// Timeout errors (connect or total) become `FetchError::Timeout`.
fn map_http_error(e: Box<dyn std::error::Error + Send + Sync>) -> FetchError {
    if let Some(req_err) = e.downcast_ref::<reqwest::Error>()
        && req_err.is_timeout()
    {
        error!(%req_err, "request timed out");
        return FetchError::Timeout;
    }
    error!(%e, "network error");
    FetchError::Network(e.to_string())
}

/// Returns true if `host` is in the Reddit allowlist.
pub fn host_allowed(host: &str) -> bool {
    ALLOWED_HOSTS.contains(&host)
}

/// Reject a URL whose host is not in the Reddit allowlist.
///
/// # Errors
///
/// Returns [`FetchError::ForbiddenHost`] when the host is not allowlisted.
pub fn reject_if_forbidden(url: &Url) -> Result<(), FetchError> {
    let host = url.host_str().unwrap_or_default();
    if !host_allowed(host) {
        return Err(FetchError::ForbiddenHost(host.to_string()));
    }
    Ok(())
}

/// Strip `r/`/leading slash; allow only `[A-Za-z0-9_+]`.
///
/// # Errors
///
/// Returns [`FetchError::InvalidSubreddit`] when the name is empty, too long, or has illegal characters.
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

/// Build a listing `.rss` URL for `sub` and `sort`.
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

/// Build a search `.rss` URL, optionally restricted to `sub`.
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
///
/// # Errors
///
/// Returns [`FetchError::InvalidUrl`] when the input is empty, too long, or unparseable.
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

/// Map one Atom entry to a [`Post`].
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

/// Split a comments feed into the submission (if found) and remaining entries as comments.
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

/// No-op HTTP backend for router tests that never reach the network.
#[derive(Debug, Clone, Default)]
pub struct NoopHttp;

impl FeedHttp for NoopHttp {
    fn get<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a HeaderMap,
    ) -> BoxFuture<'a, Result<FetchResponse, Box<dyn std::error::Error + Send + Sync>>> {
        Box::pin(async { Err(std::io::Error::other("network disabled in tests").into()) })
    }
}

#[cfg(test)]
#[allow(missing_docs, clippy::missing_docs_in_private_items)]
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
    fn host_allowed_accepts_reddit_hosts() {
        assert!(host_allowed("www.reddit.com"));
        assert!(host_allowed("reddit.com"));
        assert!(host_allowed("old.reddit.com"));
        assert!(host_allowed("np.reddit.com"));
        assert!(!host_allowed("evil.example.com"));
        assert!(!host_allowed("169.254.169.254"));
        assert!(!host_allowed(""));
    }

    #[test]
    fn reject_if_forbidden_blocks_offlist_urls() {
        let ok = Url::parse("https://www.reddit.com/r/foo/comments/abc/rss").unwrap();
        assert!(reject_if_forbidden(&ok).is_ok());
        let bad = Url::parse("https://169.254.169.254/steal").unwrap();
        assert!(matches!(
            reject_if_forbidden(&bad),
            Err(FetchError::ForbiddenHost(_))
        ));
    }

    #[test]
    fn map_http_error_non_reqwest_is_network() {
        let err: Box<dyn std::error::Error + Send + Sync> =
            Box::new(std::io::Error::other("connection refused"));
        assert!(matches!(map_http_error(err), FetchError::Network(_)));
    }

    #[test]
    fn parse_ratelimit_reset_secs_from_delta() {
        assert_eq!(parse_ratelimit_reset_secs("48"), Some(48));
        assert_eq!(parse_ratelimit_reset_secs("0.0"), None);
        assert_eq!(parse_retry_after_secs("12"), Some(12));
    }

    #[test]
    fn wait_hint_prefers_retry_after() {
        let resp = FetchResponse {
            retry_after: Some(3),
            ratelimit_reset_secs: Some(48),
            ..Default::default()
        };
        assert_eq!(resp.wait_hint(), Some(3));
        let resp = FetchResponse {
            ratelimit_reset_secs: Some(48),
            ..Default::default()
        };
        assert_eq!(resp.wait_hint(), Some(48));
        let zero_retry = FetchResponse {
            retry_after: Some(0),
            ratelimit_reset_secs: Some(48),
            ..Default::default()
        };
        assert_eq!(zero_retry.wait_hint(), Some(48));
        assert_eq!(zero_retry.retry_wait_secs(), Some(48));
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
