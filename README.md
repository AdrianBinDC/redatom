# redatom

Rust MCP server that reads Reddit through public Atom/RSS (`.rss`) only. No OAuth, no PRAW, no HTML scrape, no `.json` API.

It returns titles, excerpts, and permalinks. It does not rank, summarize, or score. RSS is flat (~25 items, no comment trees). Fetches happen only when a tool is called.

| Tool | Description |
|---|---|
| `browse_subreddit` | Subreddit listing (`hot`, `new`, `top`, `rising`, `controversial`) |
| `search_reddit` | Sitewide or subreddit search |
| `get_post_comments` | Post plus a flat list of comments |

## Run

Local:

```sh
cargo build --release
./target/release/reddit-rss-mcp
```

Always-on HTTP (port 8085, `/mcp` + `/health`):

```sh
docker build -t redatom .
docker run --name redatom -p 8085:8085 -e REDDIT_RSS_MCP_KEY=secret redatom
```

Clients, Compose, env vars: [SETUP.md](SETUP.md). Design: [SPEC.md](SPEC.md).

```sh
cargo test
```

MIT. See `LICENSE`.
