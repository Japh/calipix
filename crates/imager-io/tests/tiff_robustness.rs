//! ImageJ's single-directory stacks (the form it uses from 4 GiB up), and
//! damaged files: whatever is wrong with a file, opening and reading it
//! returns an error rather than panicking or inventing pixels.

mod tiff_support;

use std::io::{Seek, SeekFrom, Write};
use std::time::Instant;

use imager_core::{AxisKind, DType};
use tiff_support::{
    Enc, Page, ascii, build, fixture, imagej_head, lzw, open_bytes, pattern, plane, scratch, set,
    tiles,
};

/// Tags of an ImageJ greyscale stack, without the strip tags.
fn imagej_entries(
    enc: Enc,
    width: u32,
    height: u32,
    bits: u16,
    description: &str,
) -> Vec<tiff_support::Entry> {
    vec![
        enc.longs(254, &[0]),
        enc.longs(256, &[width]),
        enc.longs(257, &[height]),
        enc.shorts(258, &[bits]),
        enc.shorts(259, &[1]),
        enc.shorts(262, &[1]),
        ascii(270, description.as_bytes()),
        enc.shorts(277, &[1]),
        enc.longs(278, &[height]),
    ]
}

#[test]
fn single_directory_stack_is_read_by_offset() {
    let (w, h, n) = (32usize, 24usize, 6usize);
    let value = |p: usize, x: usize, y: usize| (p * 1000 + x + 32 * y) as u16;
    for enc in [Enc { big_endian: false }, Enc { big_endian: true }] {
        for (description, shape, third) in [
            (
                format!("ImageJ=1.54p\nimages={n}\nslices={n}\nloop=false\n"),
                vec![w, h, n],
                AxisKind::Z,
            ),
            (
                format!("ImageJ=1.54p\nimages={n}\nframes={n}\nloop=false\n"),
                vec![w, h, n],
                AxisKind::Time,
            ),
            (
                format!(
                    "ImageJ=1.54p\nimages={n}\nchannels=2\nslices=3\nhyperstack=true\nmode=grayscale\n"
                ),
                vec![w, h, 2, 3],
                AxisKind::Channel,
            ),
            // Dimensions that do not multiply to the stack size are ignored
            // (`FileOpener.setCalibration`): a plain stack is Z.
            (
                format!("ImageJ=1.54p\nimages={n}\nchannels=4\nslices=4\n"),
                vec![w, h, n],
                AxisKind::Z,
            ),
        ] {
            let entries = imagej_entries(enc, w as u32, h as u32, 16, &description);
            let mut bytes = imagej_head(enc, &entries, (w * h * 2) as u32);
            for p in 0..n {
                bytes.extend(enc.u16_samples(&pattern(w, h, |x, y| value(p, x, y))));
            }
            let image = open_bytes("single_ifd.tif", &bytes).unwrap();
            assert_eq!(image.meta().dtype(), DType::U16);
            assert_eq!(image.meta().shape(), shape, "{description:?}");
            assert_eq!(image.meta().axes()[2].kind, third, "{description:?}");
            for p in (0..n).rev() {
                assert_eq!(
                    plane::<u16>(&image, p),
                    pattern(w, h, |x, y| value(p, x, y))
                );
            }
            // One plane short: an error, as in ImageJ.
            let message = open_bytes("single_ifd_short.tif", &bytes[..bytes.len() - 1])
                .unwrap_err()
                .to_string();
            assert!(message.contains("truncated"), "{message}");
        }
    }
}

/// Marks a file sparse where the file system needs to be told, so that the
/// test below does not write gigabytes of zeros.
fn make_sparse(path: &std::path::Path) {
    if cfg!(windows) {
        let _ = std::process::Command::new("fsutil")
            .args(["sparse", "setflag"])
            .arg(path)
            .output();
    }
}

/// Run with `cargo test -p imager-io --test tiff_robustness -- --ignored`.
/// Needs a file system with sparse files (NTFS, ext4, APFS); the file
/// occupies only a few megabytes of disk.
#[test]
#[ignore = "creates a (sparse) file larger than 4 GiB"]
fn stack_larger_than_4_gib() {
    let (w, h, n) = (2048usize, 2048usize, 560usize);
    let plane_bytes = (w * h * 2) as u64;
    let enc = Enc { big_endian: true };
    let description = format!(
        "ImageJ=1.54p\nimages={n}\nslices={n}\nunit=\\u00B5m\nspacing=2.0\nloop=false\nmin=0.0\nmax=65535.0\n"
    );
    let mut entries = imagej_entries(enc, w as u32, h as u32, 16, &description);
    entries.push(enc.rational(282, 4_000_000, 1_000_000));
    entries.push(enc.rational(283, 4_000_000, 1_000_000));
    entries.push(enc.shorts(296, &[1]));
    let head = imagej_head(enc, &entries, plane_bytes as u32);
    let total = head.len() as u64 + n as u64 * plane_bytes;
    assert!(total > 4 * (1 << 30) + (1 << 28), "{total}");

    let first = pattern(w, h, |x, y| (x * 3 + y * 5) as u16);
    let last = pattern(w, h, |x, y| (x * 7 + y * 11 + 1) as u16);
    let path = scratch("larger_than_4gib.tif");
    {
        let mut file = std::fs::File::create(&path).unwrap();
        make_sparse(&path);
        file.write_all(&head).unwrap();
        file.write_all(&enc.u16_samples(&first)).unwrap();
        file.set_len(total).unwrap();
        file.seek(SeekFrom::Start(total - plane_bytes)).unwrap();
        file.write_all(&enc.u16_samples(&last)).unwrap();
        file.sync_all().unwrap();
    }
    let result = std::panic::catch_unwind(|| {
        let started = Instant::now();
        let image = imager_io::open(&path).unwrap();
        let open_time = started.elapsed();
        assert_eq!(image.meta().shape(), &[w, h, n]);
        assert_eq!(image.meta().axes()[2].scale, 2.0);

        let started = Instant::now();
        assert_eq!(plane::<u16>(&image, 0), first);
        let first_time = started.elapsed();
        let started = Instant::now();
        assert_eq!(plane::<u16>(&image, n - 1), last);
        let last_time = started.elapsed();
        // A plane in the middle of the hole reads as zeros.
        assert!(plane::<u16>(&image, n / 2).iter().all(|&v| v == 0));
        println!(
            "file of {total} bytes ({:.2} GiB), {n} planes: open {open_time:?}, first plane {first_time:?}, last plane {last_time:?}",
            total as f64 / (1u64 << 30) as f64
        );
        assert!(open_time.as_secs_f64() < 2.0, "opening took {open_time:?}");
    });
    std::fs::remove_file(&path).unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// Opens a file and, if that works, reads every plane. Errors are fine.
fn exercise(name: &str, bytes: &[u8]) -> bool {
    match open_bytes(name, bytes) {
        Ok(image) => {
            for p in 0..image.meta().plane_count().min(64) {
                let _ = image.read_plane(p);
            }
            true
        }
        Err(_) => false,
    }
}

/// Prefix lengths to try: the start (header), a spread over the middle,
/// and the end (where these files keep their directories). Each cut costs
/// a file write, so this is a sample; `src/tiff/tests.rs` tries every
/// length in memory.
fn cuts(len: usize) -> Vec<usize> {
    let mut cuts: Vec<usize> = (0..len.min(24)).collect();
    cuts.extend((24..len).step_by((len / 40).max(1)));
    cuts.extend(len.saturating_sub(40)..len);
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

#[test]
fn truncated_imagej_file_is_an_error() {
    // The hyperstack ends with directories ImageJ never reads again; losing
    // those is harmless, losing pixels or anything before them is not.
    let bytes = std::fs::read(fixture("hyperstack_c3z4t5.tif")).unwrap();
    let pixels_end = 3178 + 60 * 1536;
    let mut lengths = cuts(bytes.len());
    lengths.extend([pixels_end - 1, pixels_end, pixels_end + 1]);
    for cut in lengths {
        let opened = exercise("truncated.tif", &bytes[..cut]);
        assert_eq!(opened, cut >= pixels_end, "cut to {cut}");
    }
    let message = open_bytes("truncated.tif", &bytes[..pixels_end - 1])
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("truncated.tif") && message.contains("truncated"),
        "{message}"
    );
}

fn tiled_lzw(enc: Enc, big: bool) -> Vec<u8> {
    let (w, h) = (70usize, 50usize);
    let data = pattern(w, h, |x, y| (x * 601 + y * 257) as u16);
    let chunks = tiles(&enc.u16_samples(&data), w, h, 2, 32, 16)
        .iter()
        .map(|t| lzw(t))
        .collect();
    let mut entries = enc.gray(w as u32, h as u32, 16, 1);
    set(&mut entries, enc.shorts(259, &[5]));
    entries.push(enc.longs(322, &[32]));
    entries.push(enc.longs(323, &[16]));
    entries.push(ascii(270, b"a tiled test image"));
    build(
        enc,
        big,
        &[Page {
            entries,
            chunks,
            tiled: true,
        }],
    )
}

fn pages(enc: Enc) -> Vec<u8> {
    let pages: Vec<Page> = (0..4)
        .map(|p| {
            let data = pattern(40, 30, |x, y| (p * 1000 + x + y) as u16);
            let mut entries = enc.gray(40, 30, 16, 1);
            set(&mut entries, enc.shorts(259, &[5]));
            Page::strip(entries, lzw(&enc.u16_samples(&data)))
        })
        .collect();
    build(enc, false, &pages)
}

#[test]
fn truncated_files_of_other_kinds_never_panic() {
    for (name, bytes) in [
        ("tiled", tiled_lzw(Enc { big_endian: true }, true)),
        ("tiled", tiled_lzw(Enc { big_endian: false }, false)),
        ("pages", pages(Enc { big_endian: false })),
        ("pages", pages(Enc { big_endian: true })),
    ] {
        assert!(
            exercise("truncated_other.tif", &bytes),
            "{name} itself opens"
        );
        // These files end with their (last) directory: any cut loses it.
        for cut in cuts(bytes.len()) {
            assert!(
                !exercise("truncated_other.tif", &bytes[..cut]),
                "{name} cut to {cut}"
            );
        }
    }
}

#[test]
fn files_that_are_not_tiff() {
    for (bytes, expected) in [
        (&b""[..], "too short"),
        (&b"II\x2a\x00"[..], "too short"),
        (&b"II\x2a\x00\x00\x00\x00\x00"[..], "directory"),
        (&b"MM\x00\x2a\x00\x00\x00\x08"[..], "directory"),
        (&b"II\x2b\x00\x08\x00\x00\x00"[..], "BigTIFF"),
        (&b"II\x2c\x00\x08\x00\x00\x00"[..], "version"),
        (&b"XX\x2a\x00\x08\x00\x00\x00"[..], "signature"),
    ] {
        let message = open_bytes("not_really.tif", bytes).unwrap_err().to_string();
        assert!(message.contains(expected), "{bytes:?}: {message}");
        assert!(message.contains("not_really.tif"), "{message}");
    }
    // A directory chain that loops back on itself.
    let mut looped = pages(Enc { big_endian: false });
    let first = u32::from_le_bytes(looped[4..8].try_into().unwrap());
    let end = looped.len();
    looped[end - 4..].copy_from_slice(&first.to_le_bytes());
    let message = open_bytes("looped.tif", &looped).unwrap_err().to_string();
    assert!(message.contains("loops"), "{message}");
}
