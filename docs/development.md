# Development notes

## Layout and ownership

Crates depend downward only: `calipix-core` at the bottom, then
`calipix-roi`/`calipix-io`/`calipix-ops`/`calipix-render`, then
`calipix-command`, `calipix-commands`, `calipix-view`, and `calipix-app` on top.
`calipix-ops` knows nothing about commands; `calipix-command` knows nothing
about the GUI toolkit.

The public APIs of `calipix-core` and `calipix-command` are shared by
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

Every user-facing operation is a command in `calipix-commands`, defined once
with `CommandSpec` and a `#[derive(Params)]` struct. The conformance test in
`crates/calipix-commands/tests/standard.rs` checks that each command is
usable from every frontend. Titles, summaries, ids and menu placement follow
`docs/ux.md`.

## Commands that change samples

A command that changes samples embeds `PlaneScope`
(`crates/calipix-commands/src/process.rs`) in its parameters with
`#[param(flatten)]` and calls `scope.apply(ctx, "Blurring", |plane, info| ...)`
with a function that processes one plane. The scope then applies the rules
every such command shares:

- Only the plane on view is processed unless the whole stack is asked for.
  Interfaces fill the plane from the view and ask "Whole stack?" only when
  the image has more than one plane.
- The three channels of an RGB image are always processed together.
- With an area selection, samples outside it are left unchanged. The
  function sees the whole plane and is told the selection's bounding
  rectangle, so filters can read neighbours outside the selection.

Algorithms live in `calipix-ops` and work on plain buffers; they know
nothing of selections, scopes or commands. A command that changes the pixel
type or the shape of an image replaces the whole image instead of using
`apply`.

The selection is part of the image's state (`Dataset::roi`), not a
parameter: commands read it from the workspace. Results go to tables in the
workspace (`Workspace::table`, default name `Workspace::RESULTS`). Both are
restored by undo.

## Tests

- Unit tests next to the code; integration tests in each crate's `tests/`.
- Property tests (`proptest`) for anything with an algebraic law: round
  trips, view composition, tiled equals whole.
- Interface behaviour is tested without a window: through `AppState` for
  logic, and `egui_kittest` for widgets.
