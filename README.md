# redatom

Rust MCP server that reads Reddit via public Atom/RSS (`.rss`) endpoints only. No OAuth, no PRAW, no HTML scraping, no `.json` API.

## What it does

Gathers and surfaces structured posts and comments (titles, excerpts, permalinks). It does **not** rank, score, summarize, or evaluate content — that belongs to the caller.

- **No scores or comment trees** — RSS provides flat lists.
- **~25 items max** per request (Reddit's RSS cap).
- **On-demand only** — no background poller, cron, or prefetch. Reddit is only hit when a tool is invoked.
- **WAF / error pages are errors, not empty results.**

## Tools

| Tool | Description |
|---|---|
| `browse_subreddit` | Fetch a subreddit listing (`hot`, `new`, `top`, `rising`, `controversial`) |
| `search_reddit` | Search across Reddit or within a subreddit |
| `get_post_comments` | Fetch a post and its top ~25 comments |

## Build

```sh
cargo build --release
```

Produces the binary `reddit-rss-mcp` at `target/release/reddit-rss-mcp`.

## Transports

### stdio (default)

```sh
reddit-rss-mcp
```

Reads MCP messages from stdin, writes to stdout. Suitable for local MCP clients.

### Streamable HTTP

```sh
reddit-rss-mcp --http --bind 0.0.0.0:8085
```

Serves MCP at `POST /mcp` and a liveness probe at `GET /health`.

When binding to a **non-loopback** address (e.g. `0.0.0.0`), a bearer key is required:

```sh
REDDIT_RSS_MCP_KEY=your-secret reddit-rss-mcp --http --bind 0.0.0.0:8085
```

`GET /health` never requires the key.

## Environment variables

| Variable | Meaning | Default |
|---|---|---|
| `REDDIT_RSS_MCP_KEY` | Bearer token (required for non-loopback HTTP) | — |
| `REDDIT_RSS_UA` | Override User-Agent string | built-in |
| `REDDIT_RSS_MIN_INTERVAL_MS` | Minimum ms between Reddit requests | `2000` |
| `REDDIT_RSS_LIMITER_WAIT_SECS` | Max seconds to wait for a rate-limit permit | `30` |
| `REDDIT_RSS_TIMEOUT_SECS` | Total request timeout | `20` |
| `REDDIT_RSS_CACHE_CAP` | LRU cache entries (ETag + parsed feed) | `256` |
| `REDDIT_RSS_LOG_JSON` | `1` = JSON log output on stderr | — |
| `RUST_LOG` | `tracing` filter directive | `reddit_rss_mcp=info` |

## Hermes / MCP client config

### stdio example

```yaml
mcp_servers:
  reddit-rss:
    command: reddit-rss-mcp
    args: []
    env:
      REDDIT_RSS_MIN_INTERVAL_MS: "2000"
```

### HTTP example

```yaml
mcp_servers:
  reddit-rss:
    url: http://localhost:8085/mcp
    headers:
      Authorization: "Bearer your-secret"
```

Example tool call (Hermes skill / any MCP client):

```json
{
  "tool": "browse_subreddit",
  "args": { "subreddit": "LocalLLaMA", "sort": "hot", "limit": 10 }
}
```

## Docker

```sh
docker build -t redatom .
docker run -p 8085:8085 -e REDDIT_RSS_MCP_KEY=secret redatom
```

The image targets `linux/arm64` (or `linux/amd64` via `TARGETPLATFORM`). See `Dockerfile`.

## Tests

```sh
cargo test
```

All tests use saved Atom fixtures and a fake HTTP backend — no live Reddit calls.

There is one ignored integration test that hits live Reddit:

```sh
cargo test -- live_rss_smoke -- --ignored
```

This fetches `r/rss` and verifies the parse pipeline end-to-end.

## Limitations

- ~25 items per listing (Reddit RSS cap)
- No scores, no comment threads (RSS is flat)
- On-demand fetch only — no polling or prefetch
- WAF / rate-limit / timeout errors are surfaced as errors, never empty results
- Single UA with one fallback; no rotating UA pool

## License

MIT. See `LICENSE` and `Cargo.toml`.

## Spec

See [SPEC.md](SPEC.md) for the full design: rate limiting, caching, UA policy, error taxonomy, and CLI reference.
