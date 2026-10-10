# Development notes

## Layout and ownership

Crates depend downward only: `imager-core` at the bottom, then
`imager-roi`/`imager-io`/`imager-ops`/`imager-render`, then
`imager-command`, `imager-commands`, `imager-view`, and `imager-app` on top.
`imager-ops` knows nothing about commands; `imager-command` knows nothing
about the GUI toolkit.

The public APIs of `imager-core` and `imager-command` are shared by
everything. Extend them deliberately and additively.

## Rules for every change

- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` must pass.
- No `unsafe` (the workspace forbids it), no C toolchain or cmake
  dependencies, no copyleft dependencies (`deny.toml`).
- Errors are values with messages a user can act on: say what failed, on
  what, and why. No panics on bad input files.
- Comments describe what the code does or why, for a future reader. Match
  the density and tone of the surrounding code.
- User-visible behaviour follows `docs/ux.md`.

## Matching ImageJ

Results must match ImageJ 1.x: exactly for integer results, thresholds and
counts; within 1e-5 relative error for floating point.

- Port behaviour from the ImageJ 1.x source (public domain), not from
  memory. Name the ImageJ class and method in the doc comment of the
  function that reproduces it.
- Prove it with a test against a file in `fixtures/imagej/out`. Those files
  are written by real ImageJ from the macros in `fixtures/imagej/gen`;
  `cargo xtask gen` regenerates them (needs Java and the pinned `ij.jar`;
  see `xtask/src/main.rs`). Add a macro there when a new reference is
  needed, keep outputs small, and commit the regenerated files.
- Record every checked behaviour and every intentional difference in
  `docs/compat.md`.
- Do not read GPL-licensed sources (for example `imagej-legacy`,
  Bio-Formats, most Fiji plugins).

## Commands

Every user-facing operation is a command in `imager-commands`, defined once
with `CommandSpec` and a `#[derive(Params)]` struct. The conformance test in
`crates/imager-commands/tests/standard.rs` checks that each command is
usable from every frontend. Titles, summaries, ids and menu placement follow
`docs/ux.md`.

## Tests

- Unit tests next to the code; integration tests in each crate's `tests/`.
- Property tests (`proptest`) for anything with an algebraic law: round
  trips, view composition, tiled equals whole.
- Interface behaviour is tested without a window: through `AppState` for
  logic, and `egui_kittest` for widgets.
