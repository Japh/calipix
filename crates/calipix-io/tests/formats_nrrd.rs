//! NRRD: round trips of every sample type and shape, and hand-written
//! headers for each header feature the reader understands.

use std::io::Write;
use std::path::PathBuf;

use calipix_core::{
    Axis, AxisKind, Buffer, ColorModel, DType, Image, ImageMeta, Pixel, RealPixel, Rect,
};
use calipix_io::{Error, open, save};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-io-nrrd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Writes `header` (which must end with the blank line) followed by `data`.
fn nrrd(name: &str, header: &str, data: &[u8]) -> PathBuf {
    let path = temp(name);
    let mut bytes = header.as_bytes().to_vec();
    bytes.extend(data);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn planes<T: Pixel>(image: &Image) -> Vec<Vec<T>> {
    (0..image.meta().plane_count())
        .map(|p| {
            image
                .read_plane(p)
                .unwrap()
                .try_slice::<T>()
                .unwrap()
                .to_vec()
        })
        .collect()
}

fn kinds(image: &Image) -> Vec<AxisKind> {
    image.meta().axes().iter().map(|a| a.kind.clone()).collect()
}

fn scales(image: &Image) -> Vec<f64> {
    image.meta().axes().iter().map(|a| a.scale).collect()
}

fn units(image: &Image) -> Vec<Option<&str>> {
    image
        .meta()
        .axes()
        .iter()
        .map(|a| a.unit.as_deref())
        .collect()
}

fn open_error(path: &PathBuf) -> String {
    match open(path) {
        Err(e) => {
            assert!(matches!(e, Error::Decode { .. }), "{e:?}");
            e.to_string()
        }
        Ok(_) => panic!("{} opened", path.display()),
    }
}

// Round trips

fn assert_round_trip(image: &Image, name: &str) {
    let path = temp(name);
    save(image, &path).unwrap();
    let back = open(&path).unwrap();
    assert_eq!(back.meta().name, name);
    assert_eq!(back.meta().dtype(), image.meta().dtype(), "{name}");
    assert_eq!(back.meta().shape(), image.meta().shape(), "{name}");
    assert_eq!(back.meta().axes(), image.meta().axes(), "{name}");
    assert_eq!(back.meta().color, image.meta().color, "{name}");
    assert_eq!(back.meta().properties, image.meta().properties, "{name}");
    for plane in 0..image.meta().plane_count() {
        assert_eq!(
            back.read_plane(plane).unwrap(),
            image.read_plane(plane).unwrap(),
            "{name}, plane {plane}"
        );
    }
}

/// A C x Z x T image whose samples cover the type's whole range.
fn hyperstack<T: RealPixel>(lo: f64, hi: f64) -> Image {
    let (w, h, c, z, t) = (5, 4, 2, 3, 2);
    let meta = ImageMeta::xyczt(T::DTYPE, w, h, c, z, t).unwrap();
    let count = c * z * t;
    let buffers = (0..count)
        .map(|p| {
            let data: Vec<T> = (0..w * h)
                .map(|i| {
                    let step = (p * w * h + i) as f64 / (count * w * h - 1) as f64;
                    T::from_f64(lo * (1.0 - step) + hi * step)
                })
                .collect();
            Buffer::from_vec(data)
        })
        .collect();
    Image::from_planes(meta, buffers).unwrap()
}

#[test]
fn every_sample_type_round_trips() {
    macro_rules! check {
        ($($t:ty),*) => {$(
            let name = <$t as Pixel>::DTYPE.name();
            let (lo, hi) = (<$t>::MIN as f64, <$t>::MAX as f64);
            assert_round_trip(&hyperstack::<$t>(lo, hi), &format!("hyper_{name}.nrrd"));
            let extremes = Image::from_vec(2, 2, vec![<$t>::MIN, <$t>::MAX, 0 as $t, 1 as $t]).unwrap();
            assert_round_trip(&extremes, &format!("extremes_{name}.nrrd"));
        )*};
    }
    check!(u8, i8, u16, i16, u32, i32, u64, i64, f32, f64);

    let special = Image::from_vec(3, 1, vec![f32::INFINITY, f32::NEG_INFINITY, -0.0]).unwrap();
    assert_round_trip(&special, "special_f32.nrrd");
    let path = temp("nan.nrrd");
    save(&Image::from_vec(2, 1, vec![f64::NAN, 1.5]).unwrap(), &path).unwrap();
    let back = planes::<f64>(&open(&path).unwrap());
    assert!(back[0][0].is_nan());
    assert_eq!(back[0][1], 1.5);
}

#[test]
fn shapes_round_trip() {
    let plane = Image::from_vec(3, 2, vec![1u16, 2, 3, 4, 5, 6]).unwrap();
    assert_round_trip(&plane, "plane.nrrd");

    for (c, z, t) in [
        (1, 4, 1),
        (3, 1, 1),
        (1, 1, 5),
        (2, 3, 1),
        (1, 3, 2),
        (2, 1, 3),
    ] {
        let meta = ImageMeta::xyczt(DType::I16, 4, 3, c, z, t).unwrap();
        let buffers = (0..c * z * t)
            .map(|p| Buffer::from_vec((0..12).map(|i| (p * 100 + i) as i16 - 50).collect()))
            .collect();
        let image = Image::from_planes(meta, buffers).unwrap();
        assert_round_trip(&image, &format!("c{c}z{z}t{t}.nrrd"));
    }

    let mut meta = ImageMeta::xyczt(DType::U8, 4, 3, 3, 2, 1).unwrap();
    meta.color = ColorModel::Rgb;
    let buffers = (0..6)
        .map(|p| Buffer::from_vec((0..12).map(|i| (p * 20 + i) as u8).collect()))
        .collect();
    assert_round_trip(
        &Image::from_planes(meta, buffers).unwrap(),
        "rgb_stack.nrrd",
    );
}

#[test]
fn calibration_units_origin_and_properties_round_trip() {
    let mut x = Axis::calibrated(AxisKind::X, 0.325, "µm");
    x.origin = -12.5;
    let y = Axis::calibrated(AxisKind::Y, 0.325, "µm");
    let c = Axis::new(AxisKind::Channel);
    let z = Axis::calibrated(AxisKind::Z, 2.0, "µm");
    let mut t = Axis::calibrated(AxisKind::Time, 0.1, "s");
    t.origin = 3.0;
    let lambda = Axis::calibrated(AxisKind::Other("lambda".to_string()), 10.0, "nm");
    let mut meta = ImageMeta::new(
        DType::F32,
        vec![(x, 3), (y, 2), (c, 2), (z, 2), (t, 2), (lambda, 2)],
    )
    .unwrap();
    meta.properties.insert(
        "Info".to_string(),
        "two\nlines \\ and a backslash".to_string(),
    );
    meta.properties
        .insert("acquired by".to_string(), "imager:=test".to_string());
    let buffers = (0..16)
        .map(|p| Buffer::from_vec((0..6).map(|i| p as f32 + i as f32 / 8.0).collect()))
        .collect();
    assert_round_trip(
        &Image::from_planes(meta, buffers).unwrap(),
        "calibrated.nrrd",
    );
}

#[test]
fn written_file_is_a_plain_little_endian_nrrd() {
    let mut meta = ImageMeta::xyczt(DType::U16, 2, 1, 1, 2, 1).unwrap();
    meta.axis_mut(0).unwrap().scale = 0.5;
    meta.axis_mut(0).unwrap().unit = Some("mm".to_string());
    let image = Image::from_planes(
        meta,
        vec![
            Buffer::from_vec(vec![0x0102u16, 0x0304]),
            Buffer::from_vec(vec![0x0506u16, 0x0708]),
        ],
    )
    .unwrap();
    let path = temp("layout.nrrd");
    save(&image, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let split = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
    let header = std::str::from_utf8(&bytes[..split]).unwrap();
    assert_eq!(
        header,
        "NRRD0004\ntype: uint16\ndimension: 3\nsizes: 2 1 2\nkinds: domain domain domain\n\
         labels: \"x\" \"y\" \"z\"\nspacings: 0.5 1 1\nunits: \"mm\" \"\" \"\"\n\
         endian: little\nencoding: raw"
    );
    assert_eq!(&bytes[split + 2..], [2, 1, 4, 3, 6, 5, 8, 7]);
}

#[test]
fn refuses_complex_samples() {
    let meta = ImageMeta::plane(DType::C32, 1, 1).unwrap();
    let image = Image::from_planes(meta, vec![Buffer::zeros(DType::C32, 1)]).unwrap();
    let err = save(&image, temp("complex.nrrd")).unwrap_err();
    assert!(matches!(err, Error::Unsupported { format: "NRRD", .. }));
    assert!(err.to_string().contains("complex"), "{err}");
}

#[test]
fn saving_over_a_lazily_opened_file_works() {
    let image = Image::from_vec(2, 2, vec![10u16, 20, 30, 40]).unwrap();
    let path = temp("in_place.nrrd");
    save(&image, &path).unwrap();
    let lazy = open(&path).unwrap();
    assert!(!lazy.is_in_memory());
    // Nothing has been read yet; the save must not destroy its own source.
    save(&lazy, &path).unwrap();
    assert_eq!(planes::<u16>(&open(&path).unwrap()), [[10, 20, 30, 40]]);
}

// Hand-written headers

#[test]
fn reads_raw_little_endian_stack_lazily() {
    let data: Vec<u8> = (0..24u16).flat_map(|v| (v * 1000).to_le_bytes()).collect();
    let path = nrrd(
        "raw_le.nrrd",
        "NRRD0004\n# a comment\ntype: unsigned short\ndimension: 3\nsizes: 4 3 2\n\
         spacings: 0.5 0.5 2\nunits: \"um\" \"um\" \"um\"\nendian: little\nencoding: raw\n\n",
        &data,
    );
    let image = open(&path).unwrap();
    assert!(!image.is_in_memory());
    assert_eq!(image.meta().dtype(), DType::U16);
    assert_eq!(image.meta().shape(), &[4, 3, 2]);
    assert_eq!(kinds(&image), [AxisKind::X, AxisKind::Y, AxisKind::Z]);
    assert_eq!(scales(&image), [0.5, 0.5, 2.0]);
    assert_eq!(units(&image), [Some("um"); 3]);
    assert_eq!(image.meta().name, "raw_le.nrrd");
    let expected: Vec<Vec<u16>> = vec![
        (0..12).map(|v| v * 1000).collect(),
        (12..24).map(|v| v * 1000).collect(),
    ];
    assert_eq!(planes::<u16>(&image), expected);

    // Region reads fetch only the rows they need and agree with the plane.
    let region = image.read_region(1, Rect::new(1, 1, 2, 2)).unwrap();
    assert_eq!(
        region.try_slice::<u16>().unwrap(),
        [17000, 18000, 21000, 22000]
    );
}

#[test]
fn reads_both_byte_orders_of_every_type() {
    // (type name, pixel type, bytes of the values [1, -2 or 254...])
    macro_rules! check {
        ($($name:literal => $t:ty),* $(,)?) => {$(
            let values: [$t; 4] = [1 as $t, <$t>::MAX, <$t>::MIN, 77 as $t];
            for (endian, big) in [("little", false), ("big", true)] {
                let data: Vec<u8> = values
                    .iter()
                    .flat_map(|v| if big { v.to_be_bytes().to_vec() } else { v.to_le_bytes().to_vec() })
                    .collect();
                let header = format!(
                    "NRRD0001\ntype: {}\ndimension: 2\nsizes: 2 2\nendian: {endian}\nencoding: raw\n\n",
                    $name
                );
                let file = format!("{}_{endian}.nrrd", $name.replace(' ', "_"));
                let image = open(nrrd(&file, &header, &data)).unwrap();
                assert_eq!(image.meta().dtype(), <$t as Pixel>::DTYPE, "{}", $name);
                assert_eq!(planes::<$t>(&image), [values], "{} {endian}", $name);
            }
        )*};
    }
    check!(
        "int8" => i8, "signed char" => i8, "uchar" => u8, "uint8" => u8, "unsigned char" => u8,
        "short" => i16, "int16" => i16, "ushort" => u16, "uint16_t" => u16,
        "int" => i32, "int32" => i32, "uint" => u32, "unsigned int" => u32,
        "longlong" => i64, "int64" => i64, "unsigned long long" => u64, "uint64" => u64,
        "float" => f32, "double" => f64,
    );
}

#[test]
fn eight_bit_data_needs_no_endian() {
    let path = nrrd(
        "no_endian.nrrd",
        "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 2\nencoding: raw\n\n",
        &[1, 2, 3, 4],
    );
    assert_eq!(planes::<u8>(&open(&path).unwrap()), [[1, 2, 3, 4]]);

    let path = nrrd(
        "needs_endian.nrrd",
        "NRRD0004\ntype: int16\ndimension: 2\nsizes: 1 1\nencoding: raw\n\n",
        &[1, 2],
    );
    assert!(open_error(&path).contains("endian"));
}

#[test]
fn reads_gzip_encoding() {
    let values: Vec<f32> = (0..30).map(|v| v as f32 * 0.25 - 3.0).collect();
    let raw: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
    let path = nrrd(
        "gz.nrrd",
        "NRRD0004\ntype: float\ndimension: 3\nsizes: 5 3 2\nendian: big\nencoding: gzip\n\n",
        &gzip(&raw),
    );
    let image = open(&path).unwrap();
    assert_eq!(image.meta().shape(), &[5, 3, 2]);
    assert_eq!(planes::<f32>(&image), [&values[..15], &values[15..]]);

    // `gz` is the same encoding; `byte skip` counts decompressed bytes.
    let mut padded = vec![0xee; 7];
    padded.extend(&raw);
    let path = nrrd(
        "gz_skip.nrrd",
        "NRRD0004\ntype: float\ndimension: 3\nsizes: 5 3 2\nendian: big\nencoding: gz\nbyte skip: 7\n\n",
        &gzip(&padded),
    );
    assert_eq!(
        planes::<f32>(&open(&path).unwrap()),
        [&values[..15], &values[15..]]
    );
}

#[test]
fn truncated_gzip_data_is_an_error_when_read() {
    let raw = vec![7u8; 4000];
    let compressed = gzip(&raw);
    let header = "NRRD0004\ntype: uint8\ndimension: 3\nsizes: 20 20 10\nencoding: gzip\n\n";
    let path = nrrd("gz_cut.nrrd", header, &compressed[..compressed.len() / 2]);
    let image = open(&path).unwrap();
    let err = image.read_plane(0).unwrap_err().to_string();
    assert!(err.contains("gz_cut.nrrd"), "{err}");

    // Valid gzip holding fewer samples than the header describes.
    let path = nrrd("gz_short.nrrd", header, &gzip(&raw[..1000]));
    let err = open(&path).unwrap().read_plane(9).unwrap_err().to_string();
    assert!(err.contains("ends before"), "{err}");
}

#[test]
fn reads_text_and_hex_encodings() {
    let path = nrrd(
        "text.nrrd",
        "NRRD0004\ntype: short\ndimension: 2\nsizes: 3 2\nencoding: ascii\n\n",
        b"-1 2 3\n400 -500 32767\n",
    );
    assert_eq!(
        planes::<i16>(&open(&path).unwrap()),
        [[-1, 2, 3, 400, -500, 32767]]
    );

    let path = nrrd(
        "text_float.nrrd",
        "NRRD0004\ntype: double\ndimension: 2\nsizes: 2 1\nencoding: text\n\n",
        b"1.5e3 -0.25",
    );
    assert_eq!(planes::<f64>(&open(&path).unwrap()), [[1500.0, -0.25]]);

    let path = nrrd(
        "hex.nrrd",
        "NRRD0004\ntype: uint16\ndimension: 2\nsizes: 2 2\nendian: big\nencoding: hex\n\n",
        b"0001 00ff\nabcd FFFF\n",
    );
    assert_eq!(
        planes::<u16>(&open(&path).unwrap()),
        [[1, 255, 0xabcd, 0xffff]]
    );

    let path = nrrd(
        "text_bad.nrrd",
        "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 1\nencoding: ascii\n\n",
        b"1 banana",
    );
    let err = open(&path).unwrap().read_plane(0).unwrap_err().to_string();
    assert!(err.contains("banana"), "{err}");
}

#[test]
fn bzip2_is_reported_as_unsupported() {
    let path = nrrd(
        "bz.nrrd",
        "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 2\nencoding: bzip2\n\n",
        b"BZh9",
    );
    let err = open_error(&path);
    assert!(
        err.contains("bzip2") && err.contains("not supported"),
        "{err}"
    );
}

#[test]
fn reads_detached_header() {
    let data: Vec<u8> = (0..12u8).collect();
    std::fs::write(temp("detached.raw"), &data).unwrap();
    let header = temp("detached.nhdr");
    std::fs::write(
        &header,
        "NRRD0004\ntype: uint8\ndimension: 3\nsizes: 3 2 2\nencoding: raw\ndata file: detached.raw\n",
    )
    .unwrap();
    let image = open(&header).unwrap();
    assert_eq!(image.meta().name, "detached.nhdr");
    assert_eq!(
        planes::<u8>(&image),
        [[0, 1, 2, 3, 4, 5], [6, 7, 8, 9, 10, 11]]
    );

    // The older spelling, a gzip data file, and a header with CRLF line
    // ends and a trailing blank line.
    std::fs::write(temp("detached.raw.gz"), gzip(&data)).unwrap();
    let header = temp("detached_gz.nhdr");
    std::fs::write(
        &header,
        "NRRD0004\r\ntype: uint8\r\ndimension: 3\r\nsizes: 3 2 2\r\nencoding: gzip\r\ndatafile: detached.raw.gz\r\n\r\n",
    )
    .unwrap();
    assert_eq!(
        planes::<u8>(&open(&header).unwrap()),
        [[0, 1, 2, 3, 4, 5], [6, 7, 8, 9, 10, 11]]
    );
}

#[test]
fn detached_header_problems_are_errors() {
    let header = temp("orphan.nhdr");
    std::fs::write(
        &header,
        "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 2\nencoding: raw\ndata file: nowhere.raw\n",
    )
    .unwrap();
    let err = open(&header).unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err:?}");
    assert!(err.to_string().contains("nowhere.raw"), "{err}");

    for data_file in ["LIST", "slice%03d.raw 1 10 1"] {
        let header = temp("multi.nhdr");
        std::fs::write(
            &header,
            format!("NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 2\nencoding: raw\ndata file: {data_file}\n"),
        )
        .unwrap();
        assert!(open_error(&header).contains("several files"));
    }
}

#[test]
fn line_skip_and_byte_skip_locate_the_data() {
    let samples = [9u8, 8, 7, 6];
    let header = |extra: &str| {
        format!("NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 2\nencoding: raw\n{extra}\n")
    };

    let mut data = b"first line\nsecond line\n".to_vec();
    data.extend(samples);
    let path = nrrd("line_skip.nrrd", &header("line skip: 2\n"), &data);
    assert_eq!(planes::<u8>(&open(&path).unwrap()), [samples]);

    let mut data = vec![0xaa; 5];
    data.extend(samples);
    let path = nrrd("byte_skip.nrrd", &header("byte skip: 5\n"), &data);
    assert_eq!(planes::<u8>(&open(&path).unwrap()), [samples]);

    // Lines first, then bytes.
    let mut data = b"a line\n".to_vec();
    data.extend([0xbb; 3]);
    data.extend(samples);
    let path = nrrd(
        "both_skip.nrrd",
        &header("lineskip: 1\nbyteskip: 3\n"),
        &data,
    );
    assert_eq!(planes::<u8>(&open(&path).unwrap()), [samples]);

    // -1: the data is whatever ends the file.
    let mut data = b"a binary header of unknown length \n\n\x00\x01".to_vec();
    data.extend(samples);
    let path = nrrd("tail_skip.nrrd", &header("byte skip: -1\n"), &data);
    assert_eq!(planes::<u8>(&open(&path).unwrap()), [samples]);

    let path = nrrd("skip_too_far.nrrd", &header("byte skip: 1\n"), &samples);
    assert!(open_error(&path).contains("truncated"));
    let path = nrrd("lines_too_far.nrrd", &header("line skip: 3\n"), b"one\n");
    assert!(open_error(&path).contains("line skip"));
}

#[test]
fn space_directions_give_scale_units_and_origin() {
    let path = nrrd(
        "space.nrrd",
        "NRRD0004\ntype: uint8\ndimension: 3\nsizes: 2 2 2\nspace: left-posterior-superior\n\
         space directions: (0.5,0,0) (0,-0.25,0) (0,3,4)\nspace units: \"mm\" \"mm\" \"cm\"\n\
         space origin: (10,20,30)\nencoding: raw\n\n",
        &[0; 8],
    );
    let image = open(&path).unwrap();
    assert_eq!(kinds(&image), [AxisKind::X, AxisKind::Y, AxisKind::Z]);
    // The length of each direction vector.
    assert_eq!(scales(&image), [0.5, 0.25, 5.0]);
    // An oblique axis has no single world unit or origin.
    assert_eq!(units(&image), [Some("mm"), Some("mm"), None]);
    let origins: Vec<f64> = image.meta().axes().iter().map(|a| a.origin).collect();
    assert_eq!(origins, [10.0, 20.0, 0.0]);
}

#[test]
fn a_direction_of_none_marks_an_interleaved_channel_axis() {
    // Three channels vary fastest: r0 g0 b0 r1 g1 b1 ...
    let data: Vec<u8> = (0..2 * 2 * 2)
        .flat_map(|pixel| [pixel, 100 + pixel, 200 + pixel])
        .collect();
    let path = nrrd(
        "vector.nrrd",
        "NRRD0005\ntype: uint8\ndimension: 4\nsizes: 3 2 2 2\nspace dimension: 3\n\
         space directions: none (1,0,0) (0,1,0) (0,0,2.5)\nencoding: raw\n\n",
        &data,
    );
    let image = open(&path).unwrap();
    assert_eq!(image.meta().shape(), &[2, 2, 3, 2]);
    assert_eq!(
        kinds(&image),
        [AxisKind::X, AxisKind::Y, AxisKind::Channel, AxisKind::Z]
    );
    assert_eq!(scales(&image), [1.0, 1.0, 1.0, 2.5]);
    assert_eq!(image.meta().color, ColorModel::Grayscale);
    assert_eq!(
        planes::<u8>(&image),
        [
            [0, 1, 2, 3],
            [100, 101, 102, 103],
            [200, 201, 202, 203],
            [4, 5, 6, 7],
            [104, 105, 106, 107],
            [204, 205, 206, 207],
        ]
    );
    let region = image.read_region(4, Rect::new(1, 0, 1, 2)).unwrap();
    assert_eq!(region.try_slice::<u8>().unwrap(), [105, 107]);
}

#[test]
fn kinds_decide_channel_time_and_rgb() {
    let header = |sizes: &str, kinds: &str| {
        let dimension = sizes.split(' ').count();
        format!(
            "NRRD0004\ntype: uint8\ndimension: {dimension}\nsizes: {sizes}\nkinds: {kinds}\nencoding: raw\n\n"
        )
    };

    let data: Vec<u8> = (0..4).flat_map(|p| [p, 50 + p, 90 + p]).collect();
    let image = open(nrrd(
        "rgb.nrrd",
        &header("3 2 2", "RGB-color domain domain"),
        &data,
    ))
    .unwrap();
    assert_eq!(image.meta().shape(), &[2, 2, 3]);
    assert_eq!(image.meta().color, ColorModel::Rgb);
    assert_eq!(
        planes::<u8>(&image),
        [[0, 1, 2, 3], [50, 51, 52, 53], [90, 91, 92, 93]]
    );

    // The same kind after X and Y: planar channels.
    let image = open(nrrd(
        "rgb_planar.nrrd",
        &header("2 2 3", "space space RGB-color"),
        &data,
    ))
    .unwrap();
    assert_eq!(kinds(&image), [AxisKind::X, AxisKind::Y, AxisKind::Channel]);
    assert_eq!(image.meta().color, ColorModel::Rgb);

    let image = open(nrrd(
        "time.nrrd",
        &header("2 2 3", "domain domain time"),
        &data,
    ))
    .unwrap();
    assert_eq!(kinds(&image), [AxisKind::X, AxisKind::Y, AxisKind::Time]);

    let image = open(nrrd(
        "list.nrrd",
        &header("2 2 3", "domain domain list"),
        &data,
    ))
    .unwrap();
    assert_eq!(kinds(&image), [AxisKind::X, AxisKind::Y, AxisKind::Channel]);
    assert_eq!(image.meta().color, ColorModel::Grayscale);

    let data = [0u8; 2 * 2 * 2 * 3 * 2];
    let image = open(nrrd(
        "five.nrrd",
        &header("2 2 2 3 2", "vector domain domain domain time"),
        &data,
    ))
    .unwrap();
    assert_eq!(image.meta().shape(), &[2, 2, 2, 3, 2]);
    assert_eq!(
        kinds(&image),
        [
            AxisKind::X,
            AxisKind::Y,
            AxisKind::Channel,
            AxisKind::Z,
            AxisKind::Time
        ]
    );
}

#[test]
fn labels_decide_axes_and_undescribed_axes_are_z_then_time() {
    let data = [0u8; 2 * 2 * 3 * 2];
    let header = |extra: &str| {
        format!("NRRD0004\ntype: uint8\ndimension: 4\nsizes: 2 2 3 2\n{extra}encoding: raw\n\n")
    };

    let image = open(nrrd("unlabelled.nrrd", &header(""), &data)).unwrap();
    assert_eq!(
        kinds(&image),
        [AxisKind::X, AxisKind::Y, AxisKind::Z, AxisKind::Time]
    );

    let image = open(nrrd(
        "labelled.nrrd",
        &header("labels: \"x\" \"y\" \"C\" \"Time\"\n"),
        &data,
    ))
    .unwrap();
    assert_eq!(
        kinds(&image),
        [AxisKind::X, AxisKind::Y, AxisKind::Channel, AxisKind::Time]
    );

    // A named axis that is none of the standard ones keeps its name, and
    // the unnamed axis beside it is still Z.
    let image = open(nrrd(
        "named.nrrd",
        &header("labels: \"\" \"\" \"wavelength\" \"\"\n"),
        &data,
    ))
    .unwrap();
    assert_eq!(
        kinds(&image),
        [
            AxisKind::X,
            AxisKind::Y,
            AxisKind::Other("wavelength".to_string()),
            AxisKind::Z
        ]
    );

    let path = nrrd(
        "transposed.nrrd",
        &header("labels: \"y\" \"x\" \"z\" \"t\"\n"),
        &data,
    );
    assert!(open_error(&path).contains("axis order"));
}

#[test]
fn one_dimensional_data_is_a_single_row() {
    let path = nrrd(
        "row.nrrd",
        "NRRD0004\ntype: uint8\ndimension: 1\nsizes: 5\nencoding: raw\n\n",
        &[5, 4, 3, 2, 1],
    );
    let image = open(&path).unwrap();
    assert_eq!(image.meta().shape(), &[5, 1]);
    assert_eq!(planes::<u8>(&image), [[5, 4, 3, 2, 1]]);
}

#[test]
fn axis_mins_follow_centering() {
    let path = nrrd(
        "mins.nrrd",
        "NRRD0004\ntype: uint8\ndimension: 3\nsizes: 2 2 1\nspacings: 2 4 nan\n\
         axis mins: 10 20 nan\ncenters: cell node none\nencoding: raw\n\n",
        &[0; 4],
    );
    let image = open(&path).unwrap();
    let origins: Vec<f64> = image.meta().axes().iter().map(|a| a.origin).collect();
    // Cell-centred: the first sample's centre is half a sample in.
    assert_eq!(origins, [11.0, 20.0, 0.0]);
    assert_eq!(scales(&image), [2.0, 4.0, 1.0]);
}

#[test]
fn key_value_pairs_become_properties() {
    let path = nrrd(
        "pairs.nrrd",
        "NRRD0004\n# comment: not a field\ntype: uint8\ndimension: 2\nsizes: 1 1\n\
         content: something\nencoding: raw\nmodality:=DWMRI\nnote:=line one\\nline two \\\\ end\n\
         DWMRI_gradient_0000:= 0 0 1\n\n",
        &[1],
    );
    let image = open(&path).unwrap();
    let get = |key: &str| image.meta().properties.get(key).map(String::as_str);
    assert_eq!(get("modality"), Some("DWMRI"));
    assert_eq!(get("note"), Some("line one\nline two \\ end"));
    assert_eq!(get("DWMRI_gradient_0000"), Some(" 0 0 1"));
    assert_eq!(image.meta().properties.len(), 3);
}

#[test]
fn bad_headers_are_errors() {
    let cases = [
        (
            "NRRD0009\ntype: uint8\ndimension: 1\nsizes: 1\nencoding: raw\n\n",
            "version",
        ),
        (
            "NRRD0004\ndimension: 1\nsizes: 1\nencoding: raw\n\n",
            "`type`",
        ),
        (
            "NRRD0004\ntype: uint8\nsizes: 1\nencoding: raw\n\n",
            "`dimension`",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 1\nencoding: raw\n\n",
            "`sizes`",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 1\nsizes: 1\n\n",
            "`encoding`",
        ),
        (
            "NRRD0004\ntype: block\ndimension: 1\nsizes: 1\nencoding: raw\n\n",
            "block",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 1\nencoding: raw\n\n",
            "1 sizes for 2",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 0\nencoding: raw\n\n",
            "sizes",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 2 -1\nencoding: raw\n\n",
            "sizes",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 0\nsizes:\nencoding: raw\n\n",
            "dimension",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 1\nsizes: 1\nencoding: zip\n\n",
            "zip",
        ),
        (
            "NRRD0004\ntype: int16\ndimension: 1\nsizes: 1\nendian: middle\nencoding: raw\n\n",
            "middle",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 1 1\nspacings: 1\nencoding: raw\n\n",
            "spacings",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 1 1\nspacings: 1 wide\nencoding: raw\n\n",
            "spacings",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 1 1\nspace directions: (1,0) oops\nencoding: raw\n\n",
            "space directions",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 1 1\nkinds: domain\nencoding: raw\n\n",
            "kinds",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 1\nsizes: 1\nencoding: raw\nbyte skip: x\n\n",
            "byte skip",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 1\nsizes: 1\nencoding: gzip\nbyte skip: -1\n\n",
            "byte skip",
        ),
        (
            "NRRD0004\ntype: uint8\nthis line has no separator\n\n",
            "unreadable",
        ),
        (
            "NRRD0004\ntype: uint64\ndimension: 3\nsizes: 4294967296 4294967296 4294967296\nendian: little\nencoding: raw\n\n",
            "too large",
        ),
        (
            "NRRD0004\ntype: uint8\ndimension: 2\nsizes: 100000 100000\nencoding: raw\n\n",
            "truncated",
        ),
    ];
    for (i, (header, expected)) in cases.iter().enumerate() {
        let message = open_error(&nrrd(&format!("bad_{i}.nrrd"), header, &[0]));
        assert!(message.contains(expected), "case {i}: {message}");
    }
}

#[test]
fn every_truncation_of_a_raw_file_is_an_error() {
    let image = Image::from_vec(3, 2, vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
    let path = temp("whole.nrrd");
    save(&image, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let cut = temp("cut.nrrd");
    for len in 0..bytes.len() {
        std::fs::write(&cut, &bytes[..len]).unwrap();
        assert!(open(&cut).is_err(), "truncated to {len} bytes");
    }
}
