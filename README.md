# imager2

Open scientific N-dimensional image processing 🔬 🎇 in Rust.

imager is a port of [ImageJ2](https://imagej.net/software/imagej2/) to Rust:
one application, one download, with results that match ImageJ. It is at an
early stage; see [Status](#status).

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

`imager commands <id>` describes a command's parameters, and `--json` gives a
machine-readable description of every command.

## How it is put together

Every operation is a *command* defined once, with typed parameters and
declared side effects. Menus, dialogs, the command line, macros, extensions
and agent tools are all generated from that one definition, so they cannot
drift apart, and every run is recorded for undo and reproducibility.

| Crate | Purpose |
|---|---|
| `imager-core` | N-dimensional image model: typed buffers, calibrated axes, lazy views |
| `imager-command` | Command definitions, registry, executor, undo history |
| `imager-ops` | Image processing algorithms |
| `imager-io` | File formats |
| `imager-render` | Reference renderer (samples to display colours) |
| `imager-commands` | The built-in commands |
| `imager-view` | GPU image viewer widget |
| `imager-app` | The `imager` binary: desktop interface and command line |

The crates are an internal structure; imager ships as a single binary.

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

Processing and measurement come next (milestone 2): selections, filters,
image maths, measurements and results tables.

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
automatically by rustup). `--no-default-features` on `imager-app` builds a
headless binary without the desktop interface.

## Licence

BSD 2-Clause. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
