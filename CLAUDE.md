# Satsuma

Desktop music player (Windows-first, developed on macOS) with an Android
streaming client. Roadmap and requirements: GitHub issues #1–#14 and
`Satsuma Music Player.md`.

## Stack

- Backend: Rust, Tauri 2 (`src-tauri/`). Commands live in `src-tauri/src/commands.rs`.
- Frontend: Elm 0.19.2 (`src/`), built by Vite with `vite-plugin-elm`.
  Elm talks to Tauri only through ports; the protocol is `src/Bridge.elm`
  and the JS side is `src/bridge.js`. Keep JS to that bridge.
- Toolchain via `mise.toml` (rust, node, pnpm); Elm tools are pnpm
  devDependencies. Run commands with `mise exec --` when the shell
  is not activated.

## Conventions

- Pin every dependency exactly: `save-exact` for npm (enforced by `.npmrc`),
  `=x.y.z` for Cargo.
- Music files are the source of truth for tags, ratings and grouping; the
  SQLite database is a rebuildable cache only. What the user chose rather
  than what was derived (the library folders) is mirrored to
  `settings.json` in the app data directory, so recreating the cache never
  loses it.
- Grouping tag format: `A / B / C` (Type / Volume / Vibe); parser and
  formatter live in `src-tauri/src/grouping.rs`. It is read from the standard
  ID3 `TIT1` frame (lofty `ItemKey::ContentGroup`).
- Library data flow: `scanner.rs` walks folders and skips unchanged files by
  mtime+size, `tags.rs` reads metadata with lofty, `db.rs` is the SQLite
  cache. Scan progress reaches Elm as Tauri events forwarded by `bridge.js`.
- Playback: `queue.rs` is pure logic (shuffle, repeat, stop-after, queued
  tracks) and carries the tests; `player.rs` owns the audio device on its own
  thread and answers commands sent over a channel. Nothing else touches
  rodio. Player state reaches Elm as `player://state` events.
- rodio 0.22 renamed its types: `Player` is the old `Sink`, `MixerDeviceSink`
  the old `OutputStream`. Decode with `Decoder::try_from(File)`, which is the
  only constructor that supports seeking.
- UI language: English only.
- No "various artists" / compilation grouping in the library, ever.

## Testing the frontend

`tests/e2e` drives the real frontend in a browser through Playwright, with
`tests/e2e/tauri-stub.js` standing in for the Tauri host: it implements
`invoke` and the event callbacks, so the page runs unchanged. Use it to
check what commands a control sends and what the UI does with the state
events sent back. This is the layer where a renamed command or an event
that is never forwarded would otherwise pass every other test.

## Checks before committing

Run in this order: `tauri::generate_context!()` embeds `dist/`, so the Rust
checks need a frontend build first.

```sh
pnpm check
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Rust tests use `src-tauri/tests/fixtures/silence.mp3` (1 s of silence made
with ffmpeg) and write tags into temp copies of it.

## Workflow for this repo

- One GitHub issue per feature; branch `type/<issue>-<slug>`; PR body uses
  `Fixes #N`.
- Roborev runs in auto mode here, see `.claude/rules/roborev-loop.md`: fix
  what is worth fixing, re-review only while findings above low remain, then
  commit, push, wait for CI and squash merge.
- Never mention Claude, AI or LLMs in commits, PR titles/bodies or branch
  names.
