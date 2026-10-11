# Backlog

Things deliberately left out of the milestone in which they came up.

## Command system

- More than one shortcut per command (Ctrl+Shift+P for the palette is
  special-cased in the application).
- A verb for the dialog's accept button ("Blur" rather than "OK").
- File-type filters on path parameters.
- Running a command on a single plane, for dialog previews. Until then the
  Preview checkbox described in `docs/ux.md` is not shown.
- Options for file writers (JPEG quality is fixed at 85).

## File formats

- TIFF: JPEG, CCITT and Zstandard compression; 1, 4, 10 and 12-bit samples;
  multi-file OME-TIFF; pyramids (only the first image opens); compressed
  output.
- GIF: saving RGB images with more than 256 colours (needs a quantiser).
- NRRD: bzip2 encoding, multi-file data; test against files from other
  software.
- Raw import with user-supplied dimensions.

## Viewer

- A margin around the image for Fit to Window.

- Optional smoothing when zoomed out (views below 100% are point-sampled).
- Prefetching neighbouring tiles and planes.
- Rendering CPU-fallback tiles off the interface thread.

## Application

- Display labels for choice parameters, so generated dialogs show
  "3-3-2 RGB" rather than `rgb332`.
- The Display panel needs scrolling for three or more channels at the
  default window size.
- The native file dialog has not been observed on screen in testing, nor
  tried on macOS or Linux.

- On Windows the single binary is a console program, so launching it from
  Explorer also opens a console window. To be solved with the installer.
- Dragging a History entry or tab out into its own window (the docking
  library has no floating panels).
- Signed installers and automatic updates (milestone 6).

## Fixtures

- Let `cargo xtask gen` run Java harnesses, so the Brightness and Contrast
  "Auto" reference (`crates/calipix-render/tests/data/bc_auto.csv`) is
  regenerated and verified with the rest.
- Establish whether fixtures are byte-identical across operating systems
  and JDK versions.
