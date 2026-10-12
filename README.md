# Calipix

Calipers for pixels: open scientific N-dimensional image analysis 🔬 🎇 in
Rust.

Calipix is a Rust reimplementation of
[ImageJ2](https://imagej.net/software/imagej2/): one application, one
download, with results that match ImageJ and files that open in both. It is
an independent project, not affiliated with ImageJ. It is at an early stage;
see [Status](#status).

## Try it

```bash
cargo run --release -- path/to/image.png
```

opens the desktop window (you can also drop a file onto it). Everything the
application can do is also available without a window:

```bash
cargo run --release -- commands
```

```bash
cargo run --release -- run image.invert --input in.png -o out.png
```

`calipix commands <id>` describes a command's parameters, and `--json` gives a
machine-readable description of every command.

## How it is put together

Every operation is a *command* defined once, with typed parameters and
declared side effects. Menus, dialogs, the command line, macros, extensions
and agent tools are all generated from that one definition, so they cannot
drift apart, and every run is recorded for undo and reproducibility.

| Crate | Purpose |
|---|---|
| `calipix-core` | N-dimensional image model: typed buffers, calibrated axes, lazy views |
| `calipix-command` | Command definitions, registry, executor, undo history |
| `calipix-roi` | Selections: shapes, the pixels they cover, ImageJ's file format |
| `calipix-ops` | Image processing algorithms |
| `calipix-io` | File formats |
| `calipix-render` | Reference renderer (samples to display colours) |
| `calipix-commands` | The built-in commands |
| `calipix-view` | GPU image viewer widget |
| `calipix-app` | The `calipix` binary: desktop interface and command line |

The crates are an internal structure; Calipix ships as a single binary.

## Status

Milestone 1 (viewer) is in place:

- TIFF as ImageJ reads and writes it, including hyperstacks, calibration,
  lookup tables and stacks larger than memory; basic OME-TIFF; PNG, JPEG,
  GIF, BMP and NRRD.
- A GPU viewer for multi-channel, multi-dimensional and very large images,
  with a CPU fallback that gives identical results.
- A dockable workspace: image tabs, Info, Display, Histogram, History and
  Log panels, a command palette (Ctrl+K), dialogs generated from command
  definitions, undo and redo for everything.
- Display controls that never change image data: display range, auto
  contrast, lookup tables, channel modes.

Milestone 2 (measure) is in place:

- Selections of every ImageJ shape, drawn and edited on the image or given
  as text, and one list of saved regions per image. ImageJ's `.roi` and
  `.zip` files open and save.
- Filters, arithmetic, the image calculator, pixel-type conversion and
  geometry (crop, scale, rotate, flip), confined to the selection where
  ImageJ confines them.
- Every measurement in ImageJ's Set Measurements, a Results table that saves
  the same text ImageJ does, Set Scale, histograms and line profiles with
  plots.

Segmentation comes next (milestone 3): thresholds, binary operations,
particle analysis, stack operations, and a server that lets agents drive the
application.

[docs/compat.md](docs/compat.md) records how results compare with ImageJ,
[docs/ux.md](docs/ux.md) is the interface specification, and
[docs/backlog.md](docs/backlog.md) lists what has been deferred.

## Development

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Requires the Rust toolchain named in `rust-toolchain.toml` (installed
automatically by rustup). `--no-default-features` on `calipix-app` builds a
headless binary without the desktop interface.

## Licence

BSD 2-Clause. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
