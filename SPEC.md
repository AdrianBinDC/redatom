# reddit-rss-mcp

Rust MCP server: read Reddit via public Atom/RSS (`.rss`) only. No OAuth, PRAW, HTML scrape, or `.json`.

MIT. Clean-room rewrite of the three-tool idea from ninjackster/reddit-rss-mcp — do not transpile `server.js`.

Target: Linux aarch64. One binary, two transports (stdio + Streamable HTTP).

**Identity (do not ask):** GitHub repo is `AdrianBinDC/redatom`. Cargo package **and** binary name: `reddit-rss-mcp`. Honest UA: `reddit-rss-mcp/0.1 (+https://github.com/AdrianBinDC/redatom)`. Commit `Cargo.lock` (this is a binary). CLI flags via `clap`. HTTP tests: a small `FeedHttp` trait (or equivalent) with a fake adapter; `reqwest` is the production adapter. Dev-dep mock server is fine. Edition 2021 if 2024 is awkward on the toolchain.

Unattended operator rules: `AGENTS.md`. Scratch experiments: `tmp/` (gitignored).

**Priority: do not get the operator IP blocked.** Every Reddit GET goes through a process-wide `governor` limiter in `client` (see Rate limit). Ship the limiter in the first `client` slice, not as a later hardening pass. MCP/tools must not bypass it.

**Priority: never swallow errors; log on stderr.** Use `tracing` (see Observability). A WAF page, 429, timeout, or parse failure is a typed `FetchError` plus an error-level event — never `Ok` with `posts: []`.

**Role: gather and surface, do not evaluate.** This process is a Reddit RSS **fetch dependency**. It returns structured posts/comments (titles, excerpts, permalinks). Ranking quality, summarization, “is this worth my time,” Telegram copy, and follow-up research belong in a **caller** (Hermes skill, another agent, a human). Do not add summarize/rank/score tools, embeddings, or a digest pipeline here. Keep the JSON boring so something else can compose on it.

**Fetch is on demand only.** No background poller, cron, or prefetch inside this binary. A Reddit GET happens only when a tool is invoked (`browse_subreddit` / `search_reddit` / `get_post_comments`). `--http` may be long-lived in Docker so MCP clients can connect; while idle it must not touch Reddit (`/health` included). LRU/ETag only remember **previous on-demand** answers in RAM; they do not schedule fetches. If the operator wants a periodic temperature tick, **Hermes (or another caller)** invokes the tool on a schedule.

## Tools

Return `content: [{ type: "text", text: pretty JSON }]`.
On Reddit/client errors: MCP `CallToolResult` with `is_error: true` and a text message that includes the `FetchError` variant. Never treat a WAF/HTML page as an empty feed.

Tool descriptions must state: RSS only, no scores, no comment trees, ~25 items, no API key; and which listing surface to use (`hot`/`rising` = temperature).

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

### Listing surfaces

This server does not rank or judge. Callers pick a surface via tool args. Example sub: `LocalLLaMA` (case-insensitive).

| Job | Tool | Typical args | URL |
|---|---|---|---|
| Temperature / hot stories | `browse_subreddit` | `sort=hot` (default) | `/r/{sub}/hot.rss?limit=n` |
| Early heat | `browse_subreddit` | `sort=rising` | `/r/{sub}/rising.rss?limit=n` |
| Chronological | `browse_subreddit` | `sort=new` | `/r/{sub}/new.rss?limit=n` |
| Best in a window | `browse_subreddit` | `sort=top`, `time=day` (or `hour` `week` `month` `year` `all`) | `/r/{sub}/top.rss?limit=n&t=…` |
| Controversial in a window | `browse_subreddit` | `sort=controversial` + `time` | `/r/{sub}/controversial.rss?limit=n&t=…` |
| Several subs, one fetch | `browse_subreddit` | `subreddit=LocalLLaMA+selfhosted` | `/r/{a}+{b}/{sort}.rss` |
| Sitewide listing | `browse_subreddit` | `subreddit=all` or `popular` | `/r/all/{sort}.rss` |
| Find a topic in a sub | `search_reddit` | `subreddit` set → `restrict_sr=1` | `/r/{sub}/search.rss?q=…` |
| Find a topic sitewide | `search_reddit` | no `subreddit` | `/search.rss?q=…` |
| One thread (still shallow) | `get_post_comments` | permalink from a listing `link` | `{permalink}/.rss` |

Always ≤25 items, no scores. `time` / `t=` only apply for `top` and `controversial` on browse. No other Reddit surfaces (profiles, wiki, sidebar, `.json`).

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

struct PostSummary {
    title: String,
    author: String,
    link: String,
    published: String,
}

struct Comment {
    author: String,
    text: String, // plain text, max 500 chars
    link: String,
    published: String,
}

struct BrowseResult { source: String, count: usize, posts: Vec<Post> }
struct SearchResult { source: String, query: String, count: usize, posts: Vec<Post> }
struct CommentsResult {
    source: String,
    post: Option<PostSummary>,
    count: usize,
    comments: Vec<Comment>,
    note: String, // always the RSS limitations blurb
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
tmp/                     # gitignored scratch; not part of the crate
src/main.rs              # CLI: --stdio | --http
src/client.rs            # browse / search / comments; no rmcp types
src/error.rs
src/ua.rs
src/tools.rs             # MCP names + schemas; map args → client; JSON text
src/mcp.rs               # stdio or Streamable HTTP /mcp
tests/fixtures/*.atom
tests/client.rs
```

`client` must be testable with a fake HTTP backend and Atom fixtures. Inject or wrap the limiter so tests can assert spacing without sleeping 2s in CI (fake clock / `Quota` with a tiny period).

## Rate limit (required)

Use the [`governor`](https://docs.rs/governor) crate. Do not roll a mutex+sleep. One limiter instance is shared by stdio and HTTP.

| Constraint | Default | Env |
|---|---|---|
| Min interval between Reddit requests | 2000 ms | `REDDIT_RSS_MIN_INTERVAL_MS` |
| Max in-flight Reddit requests | 1 | (fixed) |
| Burst | 1 (no catch-up burst) | — |

Behavior:

- Acquire a `governor` permit **before** every outbound Reddit GET (including ETag revalidation). `/health` never acquires and never fetches.
- `Quota::with_period(Duration::from_millis(min_interval)).allow_burst(NonZeroU32::MIN)` (burst 1). Pair with a semaphore of 1 so overlapping tool calls cannot stampede.
- If a permit is not available immediately: **wait** up to `REDDIT_RSS_LIMITER_WAIT_SECS` (default 30). If still none → `RateLimited` (do not queue unbounded Hermes/cron calls).
- HTTP 429: honor `Retry-After` **once** (still through the limiter), then `RateLimited`. Do not tight-loop.
- HTTP 403 / `Blocked` / `NotAtom`: **no retry storm**. At most one UA fallback, then fail. `tracing::error!` with the variant.
- Do not add jittered UA rotation, parallel prefetch, or background polling. Callers that want many subs must space ticks; this process will not fire N feeds at once.

## HTTP client

- Timeouts: 10s connect, 20s total (`REDDIT_RSS_TIMEOUT_SECS` overrides total).
- `Accept: application/atom+xml, application/xml;q=0.9, text/xml;q=0.8`
- Success only if 2xx **and** (Content-Type looks like XML/Atom **or** body has Atom `<feed` root). HTML/JSON/WAF → `Blocked` / `NotAtom`.
- In-memory **LRU** of ETag / Last-Modified + last parsed result, keyed by exact request URL. `lru::LruCache`, cap `REDDIT_RSS_CACHE_CAP` (default **256**). Get/put **O(1)**. Unbounded `HashMap` is not allowed (leaks over long-lived HTTP). Send `If-None-Match` / `If-Modified-Since`. 304 → cached parse. Evict LRU on insert past cap.
- Do not retry 403 in a loop. At most one UA fallback, then `Blocked`.
- Do not fetch `.json`, `oauth.reddit.com`, or HTML listing pages.

## Observability

Use **`tracing` + `tracing-subscriber`**. Not `println!`, not `log`+`env_logger` as the app stack (libraries may emit `log`; bridge with `tracing-log` only if needed).

Init in `main` before serving:

- Writer: **stderr only**. Stdout is MCP stdio and must stay clean.
- `fmt` layer, with target/module. Default human-readable; `REDDIT_RSS_LOG_JSON=1` → JSON fields (Docker/mcp-stack).
- Filter: `RUST_LOG` (`EnvFilter`), default `reddit_rss_mcp=info,warn`.
- `#[instrument(skip_all, fields(url, status, variant))]` on client fetch paths.

Every Reddit attempt logs (info on success, error on failure): method, sanitized URL (host+path, strip query values if noisy), HTTP status, content-type, item count, cache hit (304), limiter wait ms, `FetchError` variant + `Display`. **Never log feed bodies** (PII/noise).

Errors: `client` returns `Result<_, FetchError>` end-to-end. Tools map `Err` to MCP `is_error: true` with the variant in the text. Forbidden:

- `let _ =` / `.ok()` / `unwrap_or_default()` on fetch, parse, or HTTP status
- mapping `Blocked` / `NotAtom` / `Http` / `Timeout` to an empty `posts` list
- catching a panic or error and returning a successful tool result

`clippy`: in `client` and `tools`, no `unwrap`/`expect` on HTTP, parse, or status handling. `main` may `expect` on bind/subscriber init.

## User-Agent

Sticky set of 3–5. Pick one at process start (or from env); keep for process lifetime.

1. `reddit-rss-mcp/0.1 (+https://github.com/AdrianBinDC/redatom)`
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
| `REDDIT_RSS_LIMITER_WAIT_SECS` | max wait for a permit; default `30` |
| `REDDIT_RSS_TIMEOUT_SECS` | default `20` |
| `REDDIT_RSS_CACHE_CAP` | LRU entries; default `256` |
| `RUST_LOG` | `tracing` filter; default `reddit_rss_mcp=info` |
| `REDDIT_RSS_LOG_JSON` | `1` = JSON on stderr |

## Dependencies

- `tokio`
- `rmcp` (`server`, `transport-io`, streamable-http server)
- `reqwest` + `native-tls` (OpenSSL). rustls’s TLS fingerprint is 403’d by Reddit; curl/OpenSSL is not.
- `feed-rs` for Atom (not regex)
- `serde` / `serde_json` / `schemars` as `rmcp` needs
- `thiserror`
- `governor` (required; not optional)
- `lru` (ETag + parsed-feed cache)
- `tracing`, `tracing-subscriber` (`env-filter`, `fmt`, `json`)
- `clap` (CLI)

This is a **binary**: commit `Cargo.lock`. Do not gitignore it.

Edition 2024 if toolchain supports it, else 2021.

## Complexity

No special algorithm is required on the hot path. Cost is **network + XML parse of ≤25 Atom entries**. Do not add indexes, ranking, or a custom parser.

| Path | Bound |
|---|---|
| `governor` permit + 1-in-flight semaphore | O(1) |
| Host allowlist (4 hosts) | O(1) `match` / tiny set |
| ETag + parsed-feed LRU | O(1) get/put; **O(cap)** memory, never O(urls forever) |
| Sanitize / URL build | O(input length); reject subreddit > 200 chars, query > 512, comments URL > 2048 |
| Atom → `Vec<Post>` / comments | O(entries), entries ≤ 25 (+1 submission). Linear scan to pick the post entry is enough |
| Tool JSON | O(posts) ≤ 25 |

Do not: regex-parse XML, sort/rank posts, unbounded maps, background prefetch, N parallel fetches.

## Docker (later)

linux/arm64. Multi-stage: `rust:1-bookworm` → distroless or `debian:bookworm-slim`. Command: `--http --bind 0.0.0.0:8085`. Port **8085**. Healthcheck `GET /health` on localhost, not Reddit.

## Tests (no live Reddit in CI)

1. Saved subreddit Atom → posts with titles/links.
2. Saved comments Atom → post + flat comments.
3. HTML 403/200 WAF → `Blocked` / `NotAtom`, not `posts: []`.
4. Comments URL `http://127.0.0.1/` or `http://169.254.169.254/` → `ForbiddenHost`.
5. Sanitize: `r/Foo` → `Foo`; reject `../` and hosts in the name.
6. Limit 0 and 100 → 25.
7. 304 + ETag → cached feed. LRU evicts oldest when over cap; a later fetch for an evicted URL is a full GET, not a panic.
8. Concurrent `browse` calls: at most one in-flight fake HTTP request; starts are ≥ min interval apart (use a test quota, not a 2s sleep).
9. When the limiter wait budget is 0 and no permit → `RateLimited`, no HTTP call.
10. WAF/HTML success-status body → `Err(Blocked|NotAtom)`, not `Ok` with count 0.

One ignored live test (`r/rss`) documented in README.

`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`. GitHub Actions (`.github/workflows/ci.yml`) runs those when `Cargo.toml` exists; until then the workflow is a green no-op.

## Build order

1. `client` + fixtures + errors + `governor` limiter (no MCP)
2. stdio MCP + three tools
3. HTTP + `/health` + bearer
4. Dockerfile aarch64
5. README: install, Hermes yaml, limitations

## Out of scope

Official Reddit API, scores/karma/nested trees, Camofox, HTML scrape, write tools (post/vote/comment), Telegram delivery, public unauthenticated REST API, summarization, ranking, embeddings, or “hot story” judgment.
