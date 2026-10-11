//! Builds the hand-made PNG files in `fixtures/imagej/gen/22_png_inputs`,
//! which `22_png.ijm` has ImageJ open so that `png_imagej.rs` can compare
//! what imager reads from them with what ImageJ reads.
//!
//! Between them they hold every colour type at every bit depth, palettes
//! of every kind, each form of transparency, and Adam7 interlacing. The
//! `png` crate's encoder cannot interlace, so the files are assembled here
//! chunk by chunk, with the image data in stored (uncompressed) deflate
//! blocks: their bytes then depend on nothing but this file.
//!
//! The files are committed. To rewrite them, run
//! `cargo test -p imager-io --test png_craft -- --ignored` and then
//! `cargo xtask gen`.

#[path = "png_craft/mod.rs"]
mod craft;

use craft::{crafted, input_dir};

#[test]
#[ignore = "rewrites committed fixture inputs"]
fn write_crafted_inputs() {
    std::fs::create_dir_all(input_dir()).unwrap();
    for craft in crafted() {
        std::fs::write(input_dir().join(craft.file_name()), craft.encode()).unwrap();
    }
}

/// The committed inputs are exactly what this file builds, so the builders
/// document them.
#[test]
fn crafted_inputs_are_current() {
    let all = crafted();
    for craft in &all {
        let committed = std::fs::read(input_dir().join(craft.file_name()))
            .unwrap_or_else(|e| panic!("{}: {e}; see the module documentation", craft.name));
        assert_eq!(committed, craft.encode(), "{}", craft.name);
    }
    let mut on_disk: Vec<String> = std::fs::read_dir(input_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut built: Vec<String> = all.iter().map(|c| c.file_name()).collect();
    built.sort();
    assert_eq!(on_disk, built);
}

/// The `png` crate, which knows nothing of how these files were built,
/// decodes every one to the samples it was built from.
#[test]
fn crafted_inputs_decode_to_their_samples() {
    for craft in crafted() {
        let bytes = craft.encode();
        let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
            .read_info()
            .unwrap_or_else(|e| panic!("{}: {e}", craft.name));
        let mut data = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader
            .next_frame(&mut data)
            .unwrap_or_else(|e| panic!("{}: {e}", craft.name));
        let depth = usize::from(craft.depth);
        for y in 0..craft.height {
            let row = &data[y * frame.line_size..];
            for x in 0..craft.width {
                for c in 0..craft.channels() {
                    let i = x * craft.channels() + c;
                    let stored = match depth {
                        16 => u16::from_be_bytes([row[i * 2], row[i * 2 + 1]]),
                        8 => u16::from(row[i]),
                        _ => {
                            let bit = i * depth;
                            u16::from(row[bit / 8] >> (8 - depth - bit % 8)) & ((1 << depth) - 1)
                        }
                    };
                    assert_eq!(stored, craft.at(x, y, c), "{} at {x},{y},{c}", craft.name);
                }
            }
        }
    }
}
