# Agent notes (unattended)

Follow `SPEC.md`. Do not wait for the operator.

## Autonomy

- Do not ask clarifying questions. Use the defaults in SPEC (crate name, UA URL, `clap`, commit `Cargo.lock`).
- If you are blocked on a spec contradiction or an external failure you cannot fix, open a GitHub issue (`gh issue create`) with what you tried and the exact error, then continue other unblocked work. Do not file issues for compile/clippy/fmt failures you can still fix.
- Do not invent ranking, polling, a database, disk cache, or extra MCP tools.

## Git

- Push to `main` for this MVP. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` before every commit. Never `--no-verify`.
- Do not commit `tmp/`, secrets, or feed bodies.

## Scratch

Use repo-root `tmp/` for experiments (throwaway crates, one-off parsers, raw Atom samples). It is gitignored except `tmp/.gitkeep`. Promote working code into `src/` and fixtures into `tests/fixtures/`. Delete junk in `tmp/` when done; do not prompt to keep it.

## Tests

No live Reddit in `cargo test`. Saved Atom fixtures only. One ignored live test is fine later.
