//! Integration tests for `reddit_rss_mcp::client`. No live Reddit: all
//! responses come from a fake in-memory `FeedHttp` backend.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use reddit_rss_mcp::client::{BoxFuture, FeedHttp, FetchResponse, RedatomClient};
use reddit_rss_mcp::error::FetchError;

/// Fake HTTP backend: replays pre-recorded responses from a shared queue
/// and records call counts, timestamps, and If-None-Match headers.
///
/// `responses` is `Arc<Mutex<VecDeque>>` so every call pops from the same
/// queue (sequential 304 / LRU revalidation works). The `Weak` counters let
/// the test keep observing the fake after the `RedatomClient` (which holds
/// an `Arc<dyn FeedHttp>`) is built.
struct FakeHttp {
    responses: Arc<Mutex<VecDeque<FetchResponse>>>,
    call_counter: Weak<AtomicUsize>,
    timestamps: Weak<Mutex<Vec<Instant>>>,
    if_none_match_log: Weak<Mutex<Vec<Option<String>>>>,
}

impl FakeHttp {
    fn new(
        responses: Vec<FetchResponse>,
        counter: Weak<AtomicUsize>,
        timestamps: Weak<Mutex<Vec<Instant>>>,
        inm_log: Weak<Mutex<Vec<Option<String>>>>,
    ) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            call_counter: counter,
            timestamps,
            if_none_match_log: inm_log,
        }
    }

    fn atom(body: &str) -> FetchResponse {
        FetchResponse {
            status: 200,
            content_type: "application/atom+xml; charset=utf-8".into(),
            body: body.as_bytes().to_vec(),
            etag: Some("\"v1\"".into()),
            last_modified: Some("Wed, 22 Aug 2026 12:00:00 GMT".into()),
            retry_after: None,
        }
    }
}

impl FeedHttp for FakeHttp {
    fn get(
        &self,
        _url: &str,
        _headers: &reqwest::header::HeaderMap,
    ) -> BoxFuture<'_, Result<FetchResponse, Box<dyn std::error::Error + Send + Sync>>> {
        let responses = Arc::clone(&self.responses);
        let counter = self.call_counter.clone();
        let timestamps = self.timestamps.clone();
        let inm_log = self.if_none_match_log.clone();
        let inm_value = _headers
            .get(reqwest::header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        Box::pin(async move {
            if let Some(counter) = counter.upgrade() {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            if let Some(ts) = timestamps.upgrade() {
                ts.lock().unwrap().push(Instant::now());
            }
            if let Some(inm) = inm_log.upgrade() {
                inm.lock().unwrap().push(inm_value);
            }
            let resp = responses.lock().unwrap().pop_front().ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no more recorded responses",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
            Ok(resp)
        })
    }
}

/// Bundle of handles a test needs: the client plus observables into the
/// fake backend.
struct TestHandle {
    client: RedatomClient,
    counter: Arc<AtomicUsize>,
    if_none_match_log: Arc<Mutex<Vec<Option<String>>>>,
}

fn make_client(interval_ms: u64, cache_cap: usize, responses: Vec<FetchResponse>) -> TestHandle {
    let counter = Arc::new(AtomicUsize::new(0));
    let timestamps = Arc::new(Mutex::new(Vec::new()));
    let inm_log = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeHttp::new(
        responses,
        Arc::downgrade(&counter),
        Arc::downgrade(&timestamps),
        Arc::downgrade(&inm_log),
    );
    let client = RedatomClient::with_wait(Arc::new(fake), interval_ms, cache_cap, 30);
    TestHandle {
        client,
        counter,
        if_none_match_log: inm_log,
    }
}

// 1. Subreddit Atom -> posts with titles/links.
#[tokio::test]
async fn browse_subreddit_parses_posts() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let h = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .expect("browse succeeds");
    assert_eq!(res.count, 3);
    assert!(res.source.contains("/r/LocalLLaMA/hot.rss"));
    assert!(res.source.contains("limit=25"));
    assert_eq!(res.posts[0].title, "Hot post about LLMs");
    assert_eq!(res.posts[0].author, "/u/llm_fan");
    assert!(res.posts[0].link.contains("/comments/1vv0000/"));
    assert_eq!(res.posts[0].subreddit, "LocalLLaMA");
    assert!(res.posts[0].published.starts_with("2026-08-22T09:55:00"));
    assert!(res.posts[0].excerpt.contains("Hello world"));
    assert!(!res.posts[0].excerpt.contains('<'));
}

// 2. Comments Atom -> post + flat comments.
#[tokio::test]
async fn comments_atom_has_post_and_comments() {
    let body = std::fs::read_to_string("tests/fixtures/comments.atom").unwrap();
    let h = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = h
        .client
        .get_post_comments(
            "https://www.reddit.com/r/LocalLLaMA/comments/1vv0000/hot_post_about_llms/",
            25,
        )
        .await
        .expect("comments succeed");
    assert_eq!(res.count, 2);
    assert_eq!(res.post.as_ref().unwrap().title, "Hot post about LLMs");
    assert_eq!(res.comments[0].author, "/u/commenter1");
    assert_eq!(res.comments[0].text, "I agree with this post.");
    assert!(res.note.contains("flat list"));
    assert!(res.source.ends_with(".rss"));
}

// 3. HTML 403 -> Blocked, not posts: [].
#[tokio::test]
async fn html_403_is_blocked() {
    let waf = std::fs::read_to_string("tests/fixtures/waf.html").unwrap();
    let h = make_client(
        1,
        8,
        vec![FetchResponse {
            status: 403,
            content_type: "text/html; charset=utf-8".into(),
            body: waf.as_bytes().to_vec(),
            etag: None,
            last_modified: None,
            retry_after: None,
        }],
    );
    let err = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Blocked));
}

// 3b. 200 WAF/HTML body -> Blocked, not Ok with count 0.
#[tokio::test]
async fn waf_200_html_is_blocked() {
    let waf = std::fs::read_to_string("tests/fixtures/waf.html").unwrap();
    let h = make_client(
        1,
        8,
        vec![FetchResponse {
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            body: waf.as_bytes().to_vec(),
            etag: None,
            last_modified: None,
            retry_after: None,
        }],
    );
    let err = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Blocked));
}

// 4. Non-allowlisted host -> ForbiddenHost.
#[tokio::test]
async fn comments_forbidden_host() {
    let body = std::fs::read_to_string("tests/fixtures/comments.atom").unwrap();
    for bad in [
        "http://localhost/r/x/comments/a/b/",
        "http://169.254.169.254/r/x/comments/a/b/",
        "https://evil.example.com/r/x/comments/a/b/",
    ] {
        let h = make_client(1, 8, vec![FakeHttp::atom(&body)]);
        let err = h.client.get_post_comments(bad, 25).await.unwrap_err();
        assert!(
            matches!(err, FetchError::ForbiddenHost(_)),
            "expected ForbiddenHost for {bad}, got {err:?}"
        );
    }
}

// 5. Sanitize: r/Foo -> Foo; reject ../ and hosts in the name.
#[tokio::test]
async fn sanitize_rules() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let h = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = h
        .client
        .browse_subreddit("r/Foo", "hot", None, 5)
        .await
        .unwrap();
    assert!(res.source.contains("/r/Foo/hot.rss"));

    for bad in ["../etc", "a/b", "sub.example.com", "", "r/"] {
        let h = make_client(1, 8, vec![]);
        let err = h
            .client
            .browse_subreddit(bad, "hot", None, 5)
            .await
            .unwrap_err();
        assert!(
            matches!(err, FetchError::InvalidSubreddit(_)),
            "expected InvalidSubreddit for {bad:?}, got {err:?}"
        );
    }
}

// 6. Limit 0 and 100 -> clamped to 25 in the URL.
#[tokio::test]
async fn limit_clamped_to_1_25() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    // limit=0 clamps to 1 (URL shows limit=1, take(1) → 1 post)
    let h = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 0)
        .await
        .unwrap();
    assert_eq!(
        res.count, 1,
        "limit=0 clamps to 1, fixture has 3 entries but take(1) → 1"
    );
    assert!(
        res.source.contains("limit=1"),
        "url must request 1, got {}",
        res.source
    );

    // limit=100 clamps to 25 (URL shows limit=25, take(25) → all 3 fixture posts)
    let h = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 100)
        .await
        .unwrap();
    assert_eq!(res.count, 3, "limit=100 clamps to 25, fixture has 3 posts");
    assert!(
        res.source.contains("limit=25"),
        "url must request 25, got {}",
        res.source
    );
}

// 7. 304 + ETag -> cached feed.
#[tokio::test]
async fn etag_304_serves_cache() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let responses = vec![
        FakeHttp::atom(&body),
        FetchResponse {
            status: 304,
            content_type: String::new(),
            body: Vec::new(),
            etag: Some("\"v1\"".into()),
            last_modified: None,
            retry_after: None,
        },
    ];
    let h = make_client(1, 8, responses);
    let first = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap();
    assert!(
        h.if_none_match_log
            .lock()
            .unwrap()
            .first()
            .is_none_or(Option::is_none),
        "first call must not send If-None-Match, got {:?}",
        h.if_none_match_log.lock().unwrap().first()
    );
    let second = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap();
    assert_eq!(first.count, 3);
    assert_eq!(second.count, 3, "304 must return cached parse");
    assert_eq!(
        h.counter.load(Ordering::SeqCst),
        2,
        "exactly two HTTP calls (200 then 304)"
    );
    let inm = h.if_none_match_log.lock().unwrap();
    assert!(
        inm.first().is_none_or(Option::is_none),
        "first call must not send If-None-Match, got {:?}",
        inm.first()
    );
    assert!(
        inm.get(1).is_some_and(|v| v.as_deref() == Some("\"v1\"")),
        "second call must send the stored ETag as If-None-Match, got {:?}",
        inm.get(1)
    );
}

// 7b. LRU evicts oldest; a later fetch for an evicted URL is a full GET.
#[tokio::test]
async fn lru_evicts_oldest() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let responses = vec![
        FakeHttp::atom(&body),
        FakeHttp::atom(&body),
        FakeHttp::atom(&body),
        FakeHttp::atom(&body),
    ];
    let h = make_client(1, 2, responses);
    h.client
        .browse_subreddit("aaa", "hot", None, 25)
        .await
        .unwrap();
    h.client
        .browse_subreddit("bbb", "hot", None, 25)
        .await
        .unwrap();
    h.client
        .browse_subreddit("ccc", "hot", None, 25)
        .await
        .unwrap(); // evicts "aaa"
    let res = h
        .client
        .browse_subreddit("aaa", "hot", None, 25)
        .await
        .unwrap();
    assert_eq!(res.count, 3);
    assert_eq!(
        h.counter.load(Ordering::SeqCst),
        4,
        "four distinct GETs for cap-2 LRU with 4 different URLs"
    );
}

// 8. Concurrent browse calls: at most one in-flight; starts spaced >= min interval.
#[tokio::test]
async fn concurrent_calls_serialized_with_spacing() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let responses = vec![
        FakeHttp::atom(&body),
        FakeHttp::atom(&body),
        FakeHttp::atom(&body),
    ];

    let counter = Arc::new(AtomicUsize::new(0));
    let timestamps = Arc::new(Mutex::new(Vec::new()));
    let inm_log = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeHttp::new(
        responses,
        Arc::downgrade(&counter),
        Arc::downgrade(&timestamps),
        Arc::downgrade(&inm_log),
    );
    let client = Arc::new(RedatomClient::with_wait(Arc::new(fake), 500, 8, 30));

    let c1 = client.clone();
    let c2 = client.clone();
    let c3 = client.clone();

    let h1 = tokio::spawn(async move { c1.browse_subreddit("aaa", "hot", None, 25).await });
    let h2 = tokio::spawn(async move { c2.browse_subreddit("bbb", "hot", None, 25).await });
    let h3 = tokio::spawn(async move { c3.browse_subreddit("ccc", "hot", None, 25).await });

    let r1 = h1.await.unwrap().expect("call 1 ok");
    let r2 = h2.await.unwrap().expect("call 2 ok");
    let r3 = h3.await.unwrap().expect("call 3 ok");
    assert_eq!(r1.count, 3);
    assert_eq!(r2.count, 3);
    assert_eq!(r3.count, 3);

    let ts = timestamps.lock().unwrap();
    assert!(
        ts.len() >= 3,
        "expected at least 3 HTTP calls, got {}",
        ts.len()
    );
    let gap = ts[1] - ts[0];
    assert!(
        gap >= Duration::from_millis(450),
        "limiter spacing failed: gap between call 0 and 1 is {gap:?} (need >= 450ms)"
    );
}

// 9. When the limiter wait budget is 0 and no permit is available ->
// RateLimited, no HTTP call.
#[tokio::test]
async fn limiter_wait_zero_returns_rate_limited() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let timestamps = Arc::new(Mutex::new(Vec::new()));
    let inm_log = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeHttp::new(
        vec![FakeHttp::atom(&body)],
        Arc::downgrade(&counter),
        Arc::downgrade(&timestamps),
        Arc::downgrade(&inm_log),
    );
    let c = RedatomClient::with_wait(Arc::new(fake), 1000, 8, 0);
    let r1 = c
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap();
    assert_eq!(r1.count, 3);
    let err = c
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::RateLimited), "got {err:?}");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "second call must not reach HTTP"
    );
}

// 10b. Empty search query -> InvalidQuery without any HTTP call.
#[tokio::test]
async fn search_empty_query_is_invalid() {
    let h = make_client(1, 8, vec![]);
    let err = h
        .client
        .search_reddit("   ", None, "relevance", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::InvalidQuery));
    assert_eq!(
        h.counter.load(Ordering::SeqCst),
        0,
        "no HTTP for empty query"
    );
}

// Extra: 429 with no Retry-After -> RateLimited immediately.
#[tokio::test]
async fn http_429_no_retry_after_is_rate_limited() {
    let h = make_client(
        1,
        8,
        vec![FetchResponse {
            status: 429,
            content_type: String::new(),
            body: Vec::new(),
            etag: None,
            last_modified: None,
            retry_after: None,
        }],
    );
    let err = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::RateLimited));
    assert_eq!(
        h.counter.load(Ordering::SeqCst),
        1,
        "single HTTP call, no retry"
    );
}

// 429 with Retry-After then 200 -> Ok (one retry).
#[tokio::test]
async fn http_429_retry_after_then_200() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let h = make_client(
        1,
        8,
        vec![
            FetchResponse {
                status: 429,
                content_type: String::new(),
                body: Vec::new(),
                etag: None,
                last_modified: None,
                retry_after: Some(1),
            },
            FakeHttp::atom(&body),
        ],
    );
    let res = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap();
    assert_eq!(res.count, 3);
    assert_eq!(
        h.counter.load(Ordering::SeqCst),
        2,
        "exactly two HTTP calls (429 + retry)"
    );
}

// 429 then 429 -> RateLimited after two calls.
#[tokio::test]
async fn http_429_twice_is_rate_limited() {
    let h = make_client(
        1,
        8,
        vec![
            FetchResponse {
                status: 429,
                content_type: String::new(),
                body: Vec::new(),
                etag: None,
                last_modified: None,
                retry_after: Some(1),
            },
            FetchResponse {
                status: 429,
                content_type: String::new(),
                body: Vec::new(),
                etag: None,
                last_modified: None,
                retry_after: None,
            },
        ],
    );
    let err = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::RateLimited));
    assert_eq!(
        h.counter.load(Ordering::SeqCst),
        2,
        "exactly two HTTP calls, no more retries"
    );
}

// 429 with Retry-After: 0 -> no sleep, still retries once.
#[tokio::test]
async fn http_429_retry_after_zero_retries_immediately() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let h = make_client(
        1,
        8,
        vec![
            FetchResponse {
                status: 429,
                content_type: String::new(),
                body: Vec::new(),
                etag: None,
                last_modified: None,
                retry_after: Some(0),
            },
            FakeHttp::atom(&body),
        ],
    );
    let res = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap();
    assert_eq!(res.count, 3);
    assert_eq!(h.counter.load(Ordering::SeqCst), 2);
}

// Extra: 500 -> Http(500).
#[tokio::test]
async fn http_500_is_http_error() {
    let h = make_client(
        1,
        8,
        vec![FetchResponse {
            status: 500,
            content_type: String::new(),
            body: Vec::new(),
            etag: None,
            last_modified: None,
            retry_after: None,
        }],
    );
    let err = h
        .client
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Http(500)));
}

// Extra: search URL shape (restrict_sr, encoded query).
#[tokio::test]
async fn search_url_shape() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let h = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = h
        .client
        .search_reddit(
            "llm quant",
            Some("LocalLLaMA"),
            "relevance",
            Some("week"),
            5,
        )
        .await
        .unwrap();
    assert!(
        res.source.contains("/r/LocalLLaMA/search.rss"),
        "{}",
        res.source
    );
    assert!(res.source.contains("q=llm+quant"), "{}", res.source);
    assert!(res.source.contains("restrict_sr=1"), "{}", res.source);
    assert!(res.source.contains("limit=5"), "{}", res.source);
    assert!(res.source.contains("t=week"), "{}", res.source);
}

// One ignored live test, documented for manual runs only.
#[tokio::test]
#[ignore]
async fn live_rss_smoke() {
    let c = RedatomClient::new(
        Arc::new(reddit_rss_mcp::client::ReqwestAdapter::new(
            reddit_rss_mcp::ua::Ua::from_env(),
        )),
        2000,
        256,
    );
    let res = c.browse_subreddit("rss", "hot", None, 3).await.unwrap();
    assert!(res.count > 0);
}
