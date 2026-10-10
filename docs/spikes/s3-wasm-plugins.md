# S3: sandboxed extensions as WebAssembly components

Run 2026-10-11 on Windows 10 (i7-9750H, 16 GB), rustc 1.99.

**Result: works end to end** with no cmake, NASM or extra tools. A Rust
extension built with `cargo build --target wasm32-wasip2` is already a
component; a wasmtime host loads it, lists its commands, runs one against a
16 MiB image, and contains it when it misbehaves. 10 of 10 host tests pass.

## Versions

wasmtime and wasmtime-wasi 49.0.2 (latest stable), wit-bindgen 0.62.0.

- wasmtime: `default-features = false, features = ["runtime", "cranelift",
  "component-model", "parallel-compilation", "std"]`
- wasmtime-wasi: `default-features = false, features = ["p2"]`

## Sandbox behaviour (all run, each with a positive control)

| Misbehaviour | Outcome |
|---|---|
| Infinite loop | Interrupted by epoch deadline, and separately by fuel |
| Allocating past a 64 MiB limit | Clean trap; the host is unaffected |
| Opening files, listing directories | Fails; with a read-only grant the file reads and a `..` escape is refused |
| TCP connect | Denied; works when network is granted |
| Environment and arguments | Empty |

## Costs

| | |
|---|---|
| Binary size added by wasmtime | about 17.5 MB (17.7 MB against 0.17 MB) |
| Clean build with wasmtime | release up to 11.5 min, dev about 3 min (not measured in isolation); 5 s without |
| Extension size | 57 KB for the example |
| Compile an extension at load | about 50 ms |
| Load a precompiled extension | 0.55 ms; instantiate 0.09 ms |
| Pass a 16 MiB plane to an extension and back | about 13 ms (native copy: 5.5 ms) |
| Invert 16 MiB inside the extension | 13 ms built with `opt-level = 3` and `simd128`; 25 to 65 ms with `opt-level = "s"` |

## Findings that shape the design

1. **Keep wasmtime behind a cargo feature in its own crate**, so the rest of
   the application never names it. It dominates build time and binary size.
2. **Copy planes across the boundary for version 1, behind a resource-shaped
   API.** 13 ms per 16 MiB round trip is fine for command-style extensions.
   Model images as WIT resources with region reads and writes so extensions
   can work in tiles under their memory limit. Sharing host memory with a
   guest is not available in the component model.
3. **Cache precompiled extensions**, keyed by file hash, wasmtime version and
   engine configuration: 0.5 ms to load against 50 ms to compile. Only load
   cache files the application wrote itself.
4. **Sandbox defaults:** empty WASI context, sockets off, memory limit, epoch
   deadline re-armed before every call. After a trap, discard the instance
   and create a new one.
5. **Version the ABI in the WIT package name** (`imager:plugin@0.1.0`) and
   evolve it additively.
6. **Ship an extension template** with `opt-level = 3`, LTO and `simd128`;
   the build flags make a five-fold difference.

## Pitfalls

- WIT functions and types share a namespace, and WIT keywords used as names
  need a `%` prefix.
- A new store in a fuel or epoch enabled engine traps immediately unless
  armed.
- Rust guests using `std` always import the WASI command-line, I/O and clock
  interfaces, so the host must link WASI.
- A host function cannot re-enter the extension instance that called it.

## Not checked

An ABI mismatch failing at load, typed bulk lists (`list<u16>`), limits on
host-side buffers for guest arguments, interruption while a guest waits in a
host call, platforms other than Windows, WIT resources, long-run stability.
