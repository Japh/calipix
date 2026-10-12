# S4 and S5: ImageJ reference fixtures and the `tiff` crate

Run 2026-10-10 and 11 on Windows 10, Java 21 (Temurin), rustc 1.99, ImageJ
1.54p (`net.imagej:ij` from Maven Central, checksum verified), `tiff` 0.11.3.

## S5: generating reference fixtures from ImageJ

**Result: works, but not in true headless mode.** The generator is now
`cargo xtask gen`, and its output is committed in `fixtures/imagej/out`.

- With `-Djava.awt.headless=true`, every `run("...")`, `open()`, non-TIFF
  `saveAs` and ROI Manager call throws `HeadlessException`.
- `java -jar ij.jar -batch macro.ijm <outdir>` without that flag works. No
  main window is created. It needs a display, so Linux CI would need a
  virtual one (not tested); CI does not run the generator.
- The driver has to compensate for ImageJ's batch behaviour:
  - The exit code is always 0, so each macro prints a `GEN_OK` sentinel.
  - A macro error opens a modal dialog and hangs, so there is a timeout.
  - User preferences leak in (they change the TIFF byte order, for example),
    so `user.home` and the working directory point at an empty directory.
  - Text output uses the platform line ending, so `-Dline.separator` is set.
  - `run("Measure")` never accumulates rows without a Results window, so
    fixtures use one CSV per measurement.
- Macro errors do not always announce themselves. These abort silently and
  then hang until the timeout: a comparison such as `i==0`, or an
  expression such as `arr[i]+"x"`, passed directly as a function argument
  (assign it to a variable first); a number followed by `+ userFunction()`;
  `setResult` on a row beyond the counter. Globals used inside functions
  need `var`.
- Full-precision references: `List.setMeasurements` with `List.getList`, or
  `getValue(heading)` printed with `String.format("%.17g", v)`, avoid the
  three-decimal rounding of the Results table. `saveAs("Raw Data")` keeps
  float images exact and stays out of the TIFF corpus.
- Avoid `Analyze Particles... summarize` (shows a real window), `Open
  Samples` (network) and unseeded noise.
- 43 of 44 files are byte-identical across runs on this machine. Only
  `roiset.zip` differs (zip timestamps). Cross-OS and cross-JDK identity is
  not established; regenerate on one pinned setup.

What ImageJ actually writes, worth knowing for the TIFF reader:

- 8-bit images with a colour or inverting LUT are **palette TIFFs**.
- "Signed 16-bit" is stored unsigned with an offset, plus a calibration
  function in ImageDescription (`c0=-32768.0`).
- Default byte order with no preferences is big-endian in 1.54p.
- Pixel size is in the resolution tags; unit, Z spacing and frame interval
  are only in ImageDescription.

## S4: can the `tiff` crate read and write what ImageJ users have?

**Decision: wrap the crate behind our own reader and writer. Own the ImageJ
path ourselves; use the crate for other TIFFs.**

Reading the 26 ImageJ-written TIFFs: 24 decode with correct pixels, including
a 60-plane hyperstack, both byte orders, float and RGB. The two palette files
do not decode at all.

| Gap in the crate | Consequence |
|---|---|
| No palette decoding | Cannot open ImageJ images saved with a colour or inverting LUT |
| WhiteIsZero data is silently inverted | Values differ from ImageJ |
| ImageJ's single-IFD stacks (`images=N`) appear as one plane | Stacks of 4 GiB or more open as a single image |
| No 12-bit data | Some camera files unsupported |
| `read_image()` drops planes of planar files | Must use `read_image_to_buffer()` |
| Decompressors are private | Cannot reuse only its decompression |
| Encoder: native byte order, strips only, ASCII-only strings | Cannot reproduce ImageJ's files |

It handles well: BigTIFF, tiles, LZW, Deflate, PackBits, both predictors,
both byte orders, a minimal OME-TIFF, and reading single strips or tiles
without loading the file (one strip of an 11.7 MB image: 366 KB read,
4.5 ms). Tag access, including ImageJ's private tags 50838 and 50839, is
complete. Decoders can be used one per thread.

### How ImageJ lays out stacks (from `TiffEncoder` and `TiffDecoder`)

- All planes are written as one contiguous block after the first IFD.
- N IFDs are written if the whole file stays below 4 GiB; otherwise exactly
  one, and the file is still classic TIFF.
- On reading, once `images=N` is present ImageJ ignores every further IFD
  and reads plane k at `first strip offset + k * plane bytes`.

A trap follows from this: the crate's high-level encoder places each IFD
after its plane, so the planes are not contiguous. Adding `images=N` to such
a file made ImageJ open it with the right dimensions and 59 of 60 planes
wrong. A hand-written ImageJ-layout writer of about 60 lines round-trips
correctly in real ImageJ, including the single-IFD form.

### Plan for the TIFF reader and writer (M1)

1. Open with the crate and parse the first IFD.
2. If ImageDescription starts with `ImageJ=` and the data is uncompressed,
   read planes ourselves by offset. This matches ImageJ, covers stacks over
   4 GiB and palette files, gives constant-time plane access and is
   thread-safe.
3. Send everything else through the crate, one decoder per thread, always
   behind a `BufReader` (25 times faster when walking IFDs).
4. Correct for the crate where it differs from ImageJ (WhiteIsZero, planar
   files, non-UTF-8 descriptions), and report unsupported cases clearly.
5. Write ImageJ-flavoured TIFF with our own encoder.
6. Keep crate types out of the public interface so the backend can be
   replaced.

## Not checked

A real file over 4 GB (the single-IFD case was simulated at small size);
JPEG, CCITT and ZSTD compression; 1-bit and 16-bit float data; pyramids;
OME-TIFF written by other software; whether a window briefly appears during
generation (checked only from inside the JVM); Linux and macOS.
