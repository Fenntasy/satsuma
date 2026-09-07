# Satsuma

Desktop music player (Windows-first, developed on macOS) with an Android
streaming client. Roadmap and requirements: GitHub issues #1–#14 and
`Satsuma Music Player.md`.

## Stack

- Backend: Rust, Tauri 2 (`src-tauri/`). Commands live in `src-tauri/src/commands.rs`.
- Frontend: Elm 0.19.2 (`src/`), built by Vite with `vite-plugin-elm`.
  Elm talks to Tauri only through ports; the protocol is `src/Bridge.elm`
  and the JS side is `src/bridge.js`. Keep JS to that bridge.
- Toolchain via `mise.toml`; run commands with `mise exec --` when the shell
  is not activated.

## Conventions

- Pin every dependency exactly: `save-exact` for npm (enforced by `.npmrc`),
  `=x.y.z` for Cargo.
- Music files are the source of truth for tags, ratings and grouping; the
  SQLite database is a rebuildable cache only.
- Grouping tag format: `A / B / C` (Type / Volume / Vibe), see issue #2.
- UI language: English only.
- No "various artists" / compilation grouping in the library, ever.

## Checks before committing

```sh
pnpm check
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Workflow for this repo

- One GitHub issue per feature; branch `type/<issue>-<slug>`; PR body uses
  `Fixes #N`.
- Roborev runs in auto mode here: fix findings and loop until only low
  severity remains, then commit, push, wait for CI and squash merge.
- Never mention Claude, AI or LLMs in commits, PR titles/bodies or branch
  names.
