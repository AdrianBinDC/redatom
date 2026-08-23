# Agent notes (unattended)

Follow `SPEC.md`. Do not wait for the operator. Do not ask clarifying questions.

## Autonomy

- Use SPEC defaults (crate/binary `reddit-rss-mcp`, UA URL, `clap`, commit `Cargo.lock`).
- If blocked on a spec contradiction or an external failure you cannot fix, open a GitHub issue (`gh issue create`) with what you tried and the exact error, then continue other unblocked work. Do not file issues for compile/clippy/fmt failures you can still fix.
- Do not invent ranking, polling, a database, disk cache, or extra MCP tools.

## Docs (required)

Context7 MCP is configured. **Before writing or changing code that uses a crate**, look up that crate’s API in Context7 for the **version in `Cargo.lock`** (or `Cargo.toml` if unlocked). Do not guess `governor`, `rmcp`, `lru`, `reqwest`, `feed-rs`, `axum`, or `clap` signatures. Do not wander GitHub/random blogs when Context7 has the docs. Crate source under `~/.cargo/registry` is an acceptable backup if it matches the locked version.

## Git (required)

**Never push to `main`.** Every change is a branch + pull request:

1. `git checkout -b issue/<n>-short-slug` (or `fix/…` if no issue).
2. One issue per PR when a GitHub issue exists.
3. `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must pass. Never `--no-verify`.
4. `git push -u origin HEAD` and `gh pr create` with a short summary and test plan. Do not merge unless the operator asked.

Do not commit `tmp/`, secrets, or feed bodies.

## Scratch (`tmp/` — required, do not prompt)

When you need to experiment (throwaway crate, one-off parser, raw Atom dump, Dockerfile dry-run, `curl` notes, failed snippets):

- Write under repo-root **`tmp/`**. Create subdirs as needed (`tmp/scratch-governor/`, `tmp/atom-samples/`).
- **Do not ask the operator** whether you may create files, run local experiments, or keep scratch output. Just use `tmp/`.
- `tmp/` is gitignored except `tmp/.gitkeep`. Never `git add` scratch. Never commit it.
- Promote only what works into `src/` or `tests/fixtures/`. Delete junk in `tmp/` when done; do not prompt to keep it.

## Tests

No live Reddit in `cargo test`. Saved Atom fixtures only. The ignored `live_rss_smoke` test stays ignored in CI.
