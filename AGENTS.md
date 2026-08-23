# Agent notes (unattended)

Follow `SPEC.md`. Do not wait for the operator. Do not ask clarifying questions.

## Autonomy

- Use SPEC defaults (crate/binary `reddit-rss-mcp`, UA URL, `clap`, commit `Cargo.lock`).
- Do not invent ranking, polling, a database, disk cache, or extra MCP tools.
- Do not ask the operator. Use `tmp/` for experiments. Use Context7 for crate APIs. If you are truly blocked on one GitHub issue, skip it, `gh issue comment` with the blocker, and continue the next issue. Do not stall waiting for the operator.
- If blocked on a spec contradiction or an external failure you cannot fix (and it is not already an open issue), open a GitHub issue (`gh issue create`) with what you tried and the exact error, then continue other unblocked work. Do not file issues for compile/clippy/fmt failures you can still fix.

## Docs (required)

Context7 MCP is configured. **Before writing or changing code that uses a crate**, look up that crate’s API in Context7 for the **version in `Cargo.lock`** (or `Cargo.toml` if unlocked). Do not guess `governor`, `rmcp`, `lru`, `reqwest`, `feed-rs`, `axum`, or `clap` signatures. Do not wander GitHub/random blogs when Context7 has the docs. Crate source under `~/.cargo/registry` is an acceptable backup if it matches the locked version.

## Overnight issue queue (required)

Qwen (and other unattended agents) should close **all** open GitHub issues on this repo without waiting for morning review.

1. `gh issue list --state open` and work through **every** open issue (currently #2–#8 style remediations; include any new ones that appear).
2. **One git commit per issue.** Do not squash multiple issues into one commit. The commit message should reference the issue (e.g. `Fix FakeHttp response queue (#2).`).
3. Preferred for morning review: **one branch** (e.g. `issues/overnight`) with N commits, then **one PR** that closes all of them (`Fixes #2` … in the PR body). If a later issue conflicts badly, a follow-up PR is OK; do not stall waiting for the operator.
4. **Never push to `main`.** Do not merge unless the operator asked.
5. After each issue’s commit, `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must pass. Never leave a red commit in the stack. Never `--no-verify`.
6. Independence: do not ask the operator; use `tmp/`; Context7; if truly blocked on one issue, skip it, `gh issue comment` with the blocker, continue the next issue.

## Git (required)

**Never push to `main`.** Every change is a branch + pull request.

- Overnight issue work: follow **Overnight issue queue** above (one branch, one commit per issue, one PR preferred).
- Other work (docs-only, tooling): `git checkout -b` a descriptive branch, then `git push -u origin HEAD` and `gh pr create` with a short summary. Do not merge unless the operator asked.

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
