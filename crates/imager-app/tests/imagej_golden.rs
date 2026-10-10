//! Results compared with files written by real ImageJ 1.x. The reference
//! files live in `fixtures/imagej/out` and are regenerated with
//! `cargo xtask gen`; `manifest.toml` there records the ImageJ version.

use std::ffi::OsString;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imager-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Runs `imager run image.invert` on an ImageJ-written PNG and requires
/// every sample to equal ImageJ's own Edit > Invert result.
fn assert_invert_matches_imagej(base: &str) {
    let source = fixture(&format!("{base}.png"));
    let expected = imager_io::open(fixture(&format!("{base}_invert.png"))).unwrap();
    let produced_path = temp(&format!("{base}_invert.png"));

    let argv = ["imager", "run", "image.invert", "--input"]
        .into_iter()
        .map(OsString::from)
        .chain([
            source.into_os_string(),
            "-o".into(),
            produced_path.clone().into_os_string(),
        ]);
    imager_app::cli::main(argv).unwrap();

    let produced = imager_io::open(&produced_path).unwrap();
    assert_eq!(produced.meta().dtype(), expected.meta().dtype(), "{base}");
    assert_eq!(produced.meta().shape(), expected.meta().shape(), "{base}");
    assert_eq!(produced.meta().color, expected.meta().color, "{base}");
    for plane in 0..expected.meta().plane_count() {
        assert_eq!(
            produced.read_plane(plane).unwrap(),
            expected.read_plane(plane).unwrap(),
            "{base}, plane {plane}"
        );
    }
}

#[test]
fn invert_8_bit_matches_imagej() {
    assert_invert_matches_imagej("gray8");
}

#[test]
fn invert_16_bit_matches_imagej() {
    assert_invert_matches_imagej("gray16");
}

#[test]
fn invert_rgb_matches_imagej() {
    assert_invert_matches_imagej("rgb");
}

/// Guards against the comparison passing vacuously.
#[test]
fn fixtures_are_what_the_tests_assume() {
    let gray16 = imager_io::open(fixture("gray16.png")).unwrap();
    assert_eq!(gray16.meta().dtype(), imager_core::DType::U16);
    let inverted = imager_io::open(fixture("gray16_invert.png")).unwrap();
    assert_ne!(
        gray16.read_plane(0).unwrap(),
        inverted.read_plane(0).unwrap()
    );
    let rgb = imager_io::open(fixture("rgb.png")).unwrap();
    assert_eq!(rgb.meta().plane_count(), 3);
}
