# Satsuma

A modern desktop music player, inspired by
[Strawberry](https://github.com/strawberrymusicplayer/strawberry), with a
companion Android app that streams your library from the desktop over the local
network.

## Stack

- [Tauri 2](https://tauri.app) with a Rust backend
- [Elm](https://elm-lang.org) frontend, built by Vite, with a thin JS bridge
  (`src/bridge.js`) mapping Elm ports to Tauri commands and events

## Development

Tools are managed by [mise](https://mise.jdx.dev):

```sh
mise install
pnpm install
pnpm tauri dev
```

Checks:

```sh
pnpm check                      # elm-review, elm-format, elm-test, vite build
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Windows and Android builds are produced by CI.

## Layout

- `src/` Elm application, CSS and JS bridge
- `tests/` Elm tests (`elm-test`)
- `review/` elm-review configuration
- `src-tauri/` Rust backend and Tauri configuration
