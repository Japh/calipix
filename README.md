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
| `imager-app` | The `imager` binary: desktop interface and command line |

The crates are an internal structure; imager ships as a single binary.

## Status

Milestone 0 (foundations) is in place: the image model, the command system,
PNG reading and writing, an Invert command, the command line and a minimal
window. Next is the viewer milestone: TIFF and OME-TIFF, a GPU viewer for
large and multi-dimensional images, and the dockable workspace.

[docs/compat.md](docs/compat.md) records how results compare with ImageJ.

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
