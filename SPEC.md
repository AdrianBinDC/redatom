# reddit-rss-mcp

Rust MCP server: read Reddit via public Atom/RSS (`.rss`) only. No OAuth, PRAW, HTML scrape, or `.json`.

MIT. Clean-room rewrite of the three-tool idea from ninjackster/reddit-rss-mcp — do not transpile `server.js`.

Target: Linux aarch64. One binary, two transports (stdio + Streamable HTTP).

## Tools

Return `content: [{ type: "text", text: pretty JSON }]`.
On Reddit/client errors: MCP `CallToolResult` with `is_error: true` and a text message that includes the `FetchError` variant. Never treat a WAF/HTML page as an empty feed.

Tool descriptions must state: RSS only, no scores, no comment trees, ~25 items, no API key.

### `browse_subreddit`

| Arg | Type | Required | Notes |
|---|---|---|---|
| `subreddit` | string | yes | No `r/` prefix. Allow `all`, `popular`, `+` multis (`LocalLLaMA+selfhosted`). Sanitize: strip leading `/r/`, keep `[A-Za-z0-9_+]` only. |
| `sort` | enum | no | `hot` (default), `new`, `top`, `rising`, `controversial` |
| `time` | enum | no | `hour` `day` `week` `month` `year` `all` — query `t=` only for `top` / `controversial` |
| `limit` | number | no | Clamp 1..=25 |

```
https://www.reddit.com/r/{sub}/{sort}.rss?limit={n}[&t={time}]
```

### `search_reddit`

| Arg | Type | Required | Notes |
|---|---|---|---|
| `query` | string | yes | Non-empty after trim |
| `subreddit` | string | no | If set: `/r/{sub}/search.rss` + `restrict_sr=1` |
| `sort` | enum | no | `relevance` (default), `hot`, `top`, `new`, `comments` |
| `time` | enum | no | `t=` when provided |
| `limit` | number | no | 1..=25 |

Encode `query` with `Url::query_pairs_mut`. Never concat into the path.

```
https://www.reddit.com/search.rss?q=...&sort=...&limit=...
https://www.reddit.com/r/{sub}/search.rss?q=...&sort=...&limit=...&restrict_sr=1
```

### `get_post_comments`

| Arg | Type | Required | Notes |
|---|---|---|---|
| `url` | string | yes | Post permalink or already-`.rss` URL |
| `limit` | number | no | 1..=25 comments (exclude submission entry) |

Normalize URL:

1. If no scheme, prefix `https://www.reddit.com`
2. Strip fragment and query
3. Trailing slash, then append `.rss` unless it already ends with `.rss`

Allowlist hosts: `www.reddit.com`, `reddit.com`, `old.reddit.com`, `np.reddit.com`. Anything else → `ForbiddenHost`. Do not follow redirects off this list.

Post vs comments: prefer the entry whose `link`/`id` is the submission (`/comments/{id}/` without an extra comment id). Fallback: first Atom entry is the post, rest are comments. Always include:

```
note: "RSS comments: flat list (not threaded), no scores, ~25 max."
```

## Domain

```rust
struct Post {
    title: String,
    author: String,
    link: String,       // absolute https
    published: String,  // RFC3339 if present, else raw Atom
    id: String,
    subreddit: String,  // category label, may be empty
    excerpt: String,    // plain text, max 500 chars
}

struct BrowseResult { source: String, count: usize, posts: Vec<Post> }
struct SearchResult { source: String, query: String, count: usize, posts: Vec<Post> }
struct CommentsResult {
    source: String,
    post: Option<PostSummary>, // title, author, link, published
    count: usize,
    comments: Vec<Comment>,    // author, text, link, published
    note: &'static str,
}
```

`source` is the exact URL fetched after allowlist.

### `FetchError`

Never flatten into empty posts.

| Variant | When |
|---|---|
| `InvalidSubreddit` | sanitize failed |
| `InvalidQuery` | empty search query |
| `InvalidUrl` | comments URL missing/unparseable |
| `ForbiddenHost` | comments host not allowlisted |
| `Blocked` | HTTP 403, or 200 body that is not Atom |
| `RateLimited` | HTTP 429 (`Retry-After` if present) |
| `Http(status)` | other 4xx/5xx |
| `NotAtom` | 2xx but body is not an Atom feed |
| `Timeout` | connect/total timeout |
| `Network` | DNS, TLS, reset |
| `Parse` | Atom well-formedness failure |

## Layout

```
src/main.rs     # CLI: --stdio | --http
src/client.rs   # browse / search / comments; no rmcp types
src/error.rs
src/ua.rs
src/tools.rs    # MCP names + schemas; map args → client; JSON text
src/mcp.rs      # stdio or Streamable HTTP /mcp
tests/fixtures/*.atom
tests/client.rs
```

`client` must be testable with a fake HTTP backend and Atom fixtures.

## HTTP client

- Timeouts: 10s connect, 20s total (`REDDIT_RSS_TIMEOUT_SECS` overrides total).
- `Accept: application/atom+xml, application/xml;q=0.9, text/xml;q=0.8`
- Success only if 2xx **and** (Content-Type looks like XML/Atom **or** body has Atom `<feed` root). HTML/JSON/WAF → `Blocked` / `NotAtom`.
- In-memory ETag / Last-Modified per URL. Send `If-None-Match` / `If-Modified-Since`. 304 → last parsed result.
- Process-wide rate limit (both transports): 1 in-flight request, min ~2s between requests (`REDDIT_RSS_MIN_INTERVAL_MS`, default 2000). On 429: honor `Retry-After` once, then `RateLimited`.
- Do not retry 403 in a loop. At most one UA fallback, then `Blocked`.
- Log to stderr only (method, sanitized URL, status, content-type, count, cache hit, error). Stdout is MCP only. Do not log feed bodies.
- Do not fetch `.json`, `oauth.reddit.com`, or HTML listing pages.

## User-Agent

Sticky set of 3–5. Pick one at process start (or from env); keep for process lifetime.

1. `reddit-rss-mcp/0.1 (+https://github.com/<owner>/reddit-rss-mcp)`
2. A feed reader (e.g. `NetNewsWire/6.1`)
3. Current Firefox on Linux aarch64

`REDDIT_RSS_UA` = exact header. Optional `REDDIT_RSS_UA_SET=honest|reader|firefox`. No large rotating UA pool.

## Transports

Use `rmcp` (MCP 2025-11-25). Do not hand-roll NDJSON.

**stdio (default):** `reddit-rss-mcp` or `reddit-rss-mcp --stdio`

**HTTP:** `reddit-rss-mcp --http --bind 0.0.0.0:8085`

- `POST /mcp` Streamable HTTP
- `GET /health` → `{"ok":true}` — liveness only, never call Reddit
- If bind is not loopback, require `Authorization: Bearer <token>` (`REDDIT_RSS_MCP_KEY`). Missing/wrong → 401. Exit non-zero if `--http` binds non-loopback and key is empty.
- Default bind: `127.0.0.1:8085`

## CLI / env

| Flag / env | Meaning |
|---|---|
| `--stdio` | default |
| `--http` | Streamable HTTP |
| `--bind ADDR` | default `127.0.0.1:8085` |
| `REDDIT_RSS_MCP_KEY` | bearer (required for non-loopback HTTP) |
| `REDDIT_RSS_UA` | User-Agent override |
| `REDDIT_RSS_MIN_INTERVAL_MS` | default `2000` |
| `REDDIT_RSS_TIMEOUT_SECS` | default `20` |

## Dependencies

- `tokio`
- `rmcp` (`server`, `transport-io`, streamable-http server)
- `reqwest` + rustls (avoid `openssl-sys`)
- `feed-rs` for Atom (not regex)
- `serde` / `serde_json` / `schemars` as `rmcp` needs
- `thiserror`
- Simple mutex+Instant or `governor` for rate limit

Edition 2024 if toolchain supports it, else 2021.

## Docker (later)

linux/arm64. Multi-stage: `rust:1-bookworm` → distroless or `debian:bookworm-slim`. Command: `--http --bind 0.0.0.0:8085`. Port **8085**. Healthcheck `GET /health` on localhost, not Reddit.

## Tests (no live Reddit in CI)

1. Saved subreddit Atom → posts with titles/links.
2. Saved comments Atom → post + flat comments.
3. HTML 403/200 WAF → `Blocked` / `NotAtom`, not `posts: []`.
4. Comments URL `http://127.0.0.1/` or `http://169.254.169.254/` → `ForbiddenHost`.
5. Sanitize: `r/Foo` → `Foo`; reject `../` and hosts in the name.
6. Limit 0 and 100 → 25.
7. 304 + ETag → cached feed.

One ignored live test (`r/rss`) documented in README.

`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`.

## Build order

1. `client` + fixtures + errors (no MCP)
2. stdio MCP + three tools
3. HTTP + `/health` + bearer
4. Dockerfile aarch64
5. README: install, Hermes yaml, limitations

## Out of scope

Official Reddit API, scores/karma/nested trees, Camofox, HTML scrape, write tools (post/vote/comment), Telegram delivery, public unauthenticated REST API.
