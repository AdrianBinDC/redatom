# Setup

The binary is `reddit-rss-mcp`. It speaks **stdio** (default) or **Streamable HTTP** (`--http`). One transport per client.

Images from `docker build` live in the Docker daemon (`docker images`). There is no build-output folder in this repo. `tmp/` is only for local scratch.

## Always-on HTTP (Docker)

What the Dockerfile runs: `POST /mcp` and `GET /health` on port **8085**. Binding `0.0.0.0` requires `REDDIT_RSS_MCP_KEY`. `/health` does not.

```sh
docker build -t redatom .
docker run --name redatom --restart unless-stopped \
  -p 8085:8085 \
  -e REDDIT_RSS_MCP_KEY=secret \
  redatom
```

Point the MCP client at `http://127.0.0.1:8085/mcp` with `Authorization: Bearer secret`.

Existing Compose file: add a service that `build`s this repo, publishes `8085:8085`, sets `REDDIT_RSS_MCP_KEY`, and joins the same network as your other HTTP MCP servers.

```yaml
  redatom:
    build: . # or the path to this repo
    ports:
      - "8085:8085"
    environment:
      REDDIT_RSS_MCP_KEY: "${REDDIT_RSS_MCP_KEY}"
    restart: unless-stopped
```

The image already healthchecks `GET /health` (`{"ok":true}`, no Bearer). `/mcp` needs the key. After the client config changes, start a new session (or reload MCP); old sessions will not see the tools.

## Local stdio

```sh
cargo build --release
```

Then give the client the binary path (`target/release/reddit-rss-mcp`). No args. Logs go to stderr.

## HTTP without Docker

Loopback needs no key:

```sh
./target/release/reddit-rss-mcp --http --bind 127.0.0.1:8085
```

All interfaces:

```sh
REDDIT_RSS_MCP_KEY=secret ./target/release/reddit-rss-mcp --http --bind 0.0.0.0:8085
```

## Client config

Put the server on whichever config that session actually loads (some clients have profiles that do not inherit a default file).

HTTP:

```yaml
mcp_servers:
  redatom:
    url: http://127.0.0.1:8085/mcp
    headers:
      Accept: application/json, text/event-stream
      Authorization: Bearer secret
```

stdio:

```yaml
mcp_servers:
  redatom:
    command: /absolute/path/to/reddit-rss-mcp
    args: []
```

JSON clients use the same fields (`url` + `headers`, or `command` + `args`). Restart the client after editing.

If the session can search this repo and run a shell, it will do that instead of calling the tools. Use a session where Reddit comes only from these MCP tools.

There is no skill named redatom. Some clients hide MCP tools behind search/describe. A direct call then says the tool does not exist even though the server is up. Load the schema (`tool_describe` / equivalent) then call. Do not invent extra tool names.

Tools: `browse_subreddit`, `search_reddit`, `get_post_comments`.

```json
{
  "tool": "browse_subreddit",
  "args": { "subreddit": "LocalLLaMA", "sort": "hot", "limit": 10 }
}
```

## Environment

| Variable | Meaning | Default |
|---|---|---|
| `REDDIT_RSS_MCP_KEY` | Bearer token (required for non-loopback HTTP) | — |
| `REDDIT_RSS_UA` | User-Agent override | built-in |
| `REDDIT_RSS_MIN_INTERVAL_MS` | Min ms between Reddit requests | `2000` |
| `REDDIT_RSS_LIMITER_WAIT_SECS` | Max wait for a governor permit | `30` |
| `REDDIT_RSS_QUOTA_WAIT_SECS` | Max wait for Reddit `remaining=0` reset | `90` |
| `REDDIT_RSS_TIMEOUT_SECS` | Total request timeout | `20` |
| `REDDIT_RSS_CACHE_CAP` | LRU entries (ETag + parsed feed) | `256` |
| `REDDIT_RSS_LOG_JSON` | `1` = JSON logs on stderr | — |
| `RUST_LOG` | `tracing` filter | `reddit_rss_mcp=info` |

## Tests

```sh
cargo test
```

Fixtures only. Live Reddit:

```sh
cargo test -- live_rss_smoke -- --ignored
```
