# Agent notes (unattended)

Follow `SPEC.md`. Do not wait for the operator. Do not ask clarifying questions.

## Autonomy

- Use SPEC defaults (crate/binary `reddit-rss-mcp`, UA URL, `clap`, commit `Cargo.lock`).
- Do not invent ranking, polling, a database, disk cache, or extra MCP tools.
- Do not ask the operator. Use `tmp/` for experiments. Use Context7 for crate APIs. If you are truly blocked on one GitHub issue, skip it, `gh issue comment` with the blocker, and continue the next issue. Do not stall waiting for the operator.
- If blocked on a spec contradiction or an external failure you cannot fix (and it is not already an open issue), open a GitHub issue (`gh issue create`) with what you tried and the exact error, then continue other unblocked work. Do not file issues for compile/clippy/fmt failures you can still fix.

## Docs (required)

How to run it (stdio vs HTTP, Docker, clients, env): `SETUP.md`. Do not invent a second setup story in the README.

Context7 MCP is configured. **Before writing or changing code that uses a crate**, look up that crate’s API in Context7 for the **version in `Cargo.lock`** (or `Cargo.toml` if unlocked). Do not guess `governor`, `rmcp`, `lru`, `reqwest`, `feed-rs`, `axum`, or `clap` signatures. Do not wander GitHub/random blogs when Context7 has the docs. Crate source under `~/.cargo/registry` is an acceptable backup if it matches the locked version.

## Overnight issue queue (required)

Close **all** open GitHub issues without waiting for morning review.

**One branch only:** `issues/overnight-08-23-2026`. **One PR** from that branch. **One commit per issue.** Do not open a branch or PR per issue.

1. `git fetch origin && git checkout main && git pull && git checkout -b issues/overnight-08-23-2026` (if the branch already exists on origin, check it out and continue; do not create a second overnight branch).
2. `gh issue list --state open` and work through **every** open issue (including new ones).
3. **One git commit per issue.** Do not squash issues together. Message cites the issue, e.g. `Fix FakeHttp response queue (#2).`
4. After each commit: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must pass. Never `--no-verify`. Never leave a red commit.
5. Push the **same** branch (`git push -u origin issues/overnight-08-23-2026`). Open **one** PR targeting `main` whose body lists `Fixes #N` for every issue in the stack. If the PR already exists, push more commits onto it; do not open another PR.
6. **Never push to `main`.** Do not merge unless the operator asked.
7. If one issue is blocked: `gh issue comment` with the blocker, skip it, continue. Do not stall.

## Git (required)

**Never push to `main`.** Unattended work uses **`issues/overnight-08-23-2026` only** (see above). Do not use `issue/<n>-…` branches or one-PR-per-issue.

Before every commit: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`. Never `--no-verify`.

Do not commit `tmp/`, secrets, or feed bodies.

## Scratch (`tmp/` — required, do not prompt)

When you need to experiment (throwaway crate, one-off parser, raw Atom dump, Dockerfile dry-run, `curl` notes, failed snippets):

- Write under repo-root **`tmp/`**. Create subdirs as needed (`tmp/scratch-governor/`, `tmp/atom-samples/`).
- **Do not ask the operator** whether you may create files, run local experiments, or keep scratch output. Just use `tmp/`.
- `tmp/` is gitignored except `tmp/.gitkeep`. Never `git add` scratch. Never commit it.
- Promote only what works into `src/` or `tests/fixtures/`. Delete junk in `tmp/` when done; do not prompt to keep it.

## Tests

No live Reddit in `cargo test`. Saved Atom fixtures only. The ignored `live_rss_smoke` test stays ignored in CI.
