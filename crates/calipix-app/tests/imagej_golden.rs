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
    let dir = std::env::temp_dir().join(format!("calipix-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Runs `calipix run image.invert` on an ImageJ-written PNG and requires
/// every sample to equal ImageJ's own Edit > Invert result.
fn assert_invert_matches_imagej(base: &str) {
    let source = fixture(&format!("{base}.png"));
    let expected = calipix_io::open(fixture(&format!("{base}_invert.png"))).unwrap();
    let produced_path = temp(&format!("{base}_invert.png"));

    let argv = ["calipix", "run", "image.invert", "--input"]
        .into_iter()
        .map(OsString::from)
        .chain([
            source.into_os_string(),
            "-o".into(),
            produced_path.clone().into_os_string(),
        ]);
    calipix_app::cli::main(argv).unwrap();

    let produced = calipix_io::open(&produced_path).unwrap();
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

/// Float images: ImageJ inverts over the display range, which for a
/// freshly opened file is the data range.
#[test]
fn invert_float_matches_imagej() {
    let expected = calipix_io::open(fixture("gray32f_invert.tif")).unwrap();
    let produced_path = temp("gray32f_invert.tif");
    let argv = ["calipix", "run", "image.invert", "--input"]
        .into_iter()
        .map(OsString::from)
        .chain([
            fixture("gray32f.tif").into_os_string(),
            "-o".into(),
            produced_path.clone().into_os_string(),
        ]);
    calipix_app::cli::main(argv).unwrap();

    let produced = calipix_io::open(&produced_path).unwrap();
    assert_eq!(produced.meta().shape(), expected.meta().shape());
    let (got, want) = (
        produced.read_plane(0).unwrap(),
        expected.read_plane(0).unwrap(),
    );
    assert_eq!(
        got.as_slice::<f32>().unwrap(),
        want.as_slice::<f32>().unwrap(),
        "float inversion must be bit-identical to ImageJ"
    );
}

/// Guards against the comparison passing vacuously.
#[test]
fn fixtures_are_what_the_tests_assume() {
    let gray16 = calipix_io::open(fixture("gray16.png")).unwrap();
    assert_eq!(gray16.meta().dtype(), calipix_core::DType::U16);
    let inverted = calipix_io::open(fixture("gray16_invert.png")).unwrap();
    assert_ne!(
        gray16.read_plane(0).unwrap(),
        inverted.read_plane(0).unwrap()
    );
    let rgb = calipix_io::open(fixture("rgb.png")).unwrap();
    assert_eq!(rgb.meta().plane_count(), 3);
}
