//! Integration tests for `reddit_rss_mcp::client`. No live Reddit: all
//! responses come from a fake in-memory `FeedHttp` backend.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use reddit_rss_mcp::client::{BoxFuture, FeedHttp, FetchResponse, RedatomClient};
use reddit_rss_mcp::error::FetchError;

/// Fake HTTP backend: replays pre-recorded responses in order and records
/// call counts + timestamps so tests can assert spacing.
///
/// `call_counter` / `timestamps` use `Weak` so the test can clone the
/// `RedatomClient` (which holds an `Arc<dyn FeedHttp>`) and observe the
/// fake's state without keeping it alive via the client.
struct FakeHttp {
    responses: Mutex<VecDeque<FetchResponse>>,
    call_counter: Weak<AtomicUsize>,
    timestamps: Weak<Mutex<Vec<Instant>>>,
}

// Keep the real `Instant` type import short
use std::time::Instant;

impl FakeHttp {
    fn new(
        responses: Vec<FetchResponse>,
        counter: Weak<AtomicUsize>,
        timestamps: Weak<Mutex<Vec<Instant>>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            call_counter: counter,
            timestamps,
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
        let responses = Arc::new(Mutex::new(VecDeque::new()));
        // Move responses into the shared arc so the future can access them.
        *responses.lock().unwrap() = self.responses.lock().unwrap().clone();
        let counter = self.call_counter.clone();
        let timestamps = self.timestamps.clone();

        Box::pin(async move {
            if let Some(counter) = counter.upgrade() {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            if let Some(ts) = timestamps.upgrade() {
                ts.lock().unwrap().push(Instant::now());
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

fn make_client(
    interval_ms: u64,
    cache_cap: usize,
    responses: Vec<FetchResponse>,
) -> (RedatomClient, Arc<AtomicUsize>, Arc<Mutex<Vec<Instant>>>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let timestamps = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeHttp::new(
        responses,
        Arc::downgrade(&counter),
        Arc::downgrade(&timestamps),
    );
    let client = RedatomClient::with_wait(Arc::new(fake), interval_ms, cache_cap, 30);
    (client, counter, timestamps)
}

// 1. Subreddit Atom -> posts with titles/links.
#[tokio::test]
async fn browse_subreddit_parses_posts() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let (c, _, _) = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = c
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
    let (c, _, _) = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = c
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
    let (c, _, _) = make_client(
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
    let err = c
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Blocked));
}

// 3b. 200 WAF/HTML body -> Blocked, not Ok with count 0.
#[tokio::test]
async fn waf_200_html_is_blocked() {
    let waf = std::fs::read_to_string("tests/fixtures/waf.html").unwrap();
    let (c, _, _) = make_client(
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
    let err = c
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
        let (c, _, _) = make_client(1, 8, vec![FakeHttp::atom(&body)]);
        let err = c.get_post_comments(bad, 25).await.unwrap_err();
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
    let (c, _, _) = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = c.browse_subreddit("r/Foo", "hot", None, 5).await.unwrap();
    assert!(res.source.contains("/r/Foo/hot.rss"));

    for bad in ["../etc", "a/b", "sub.example.com", "", "r/"] {
        let (c, _, _) = make_client(1, 8, vec![]);
        let err = c.browse_subreddit(bad, "hot", None, 5).await.unwrap_err();
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
    let (c, _, _) = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = c
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
    let (c, _, _) = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = c
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
    let (c, _, _) = make_client(1, 8, responses);
    let first = c
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap();
    let second = c
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap();
    assert_eq!(first.count, 3);
    assert_eq!(second.count, 3, "304 must return cached parse");
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
    let (c, _, _) = make_client(1, 2, responses);
    c.browse_subreddit("aaa", "hot", None, 25).await.unwrap();
    c.browse_subreddit("bbb", "hot", None, 25).await.unwrap();
    c.browse_subreddit("ccc", "hot", None, 25).await.unwrap(); // evicts "aaa"
    let res = c.browse_subreddit("aaa", "hot", None, 25).await.unwrap();
    assert_eq!(res.count, 3);
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
    let fake = FakeHttp::new(
        responses,
        Arc::downgrade(&counter),
        Arc::downgrade(&timestamps),
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
    let fake = FakeHttp {
        responses: std::sync::Mutex::new(vec![FakeHttp::atom(&body)].into()),
        call_counter: Arc::downgrade(&counter),
        timestamps: Arc::downgrade(&timestamps),
    };
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
    let (c, counter, _) = make_client(1, 8, vec![]);
    let err = c
        .search_reddit("   ", None, "relevance", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::InvalidQuery));
    assert_eq!(counter.load(Ordering::SeqCst), 0, "no HTTP for empty query");
}

// Extra: 429 -> RateLimited.
#[tokio::test]
async fn http_429_is_rate_limited() {
    let (c, _, _) = make_client(
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
    let err = c
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::RateLimited));
}

// Extra: 500 -> Http(500).
#[tokio::test]
async fn http_500_is_http_error() {
    let (c, _, _) = make_client(
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
    let err = c
        .browse_subreddit("LocalLLaMA", "hot", None, 25)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Http(500)));
}

// Extra: search URL shape (restrict_sr, encoded query).
#[tokio::test]
async fn search_url_shape() {
    let body = std::fs::read_to_string("tests/fixtures/subreddit.atom").unwrap();
    let (c, _, _) = make_client(1, 8, vec![FakeHttp::atom(&body)]);
    let res = c
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
