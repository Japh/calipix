use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use calipix_core::Image;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn calipix(args: &[&str]) -> anyhow::Result<String> {
    let argv = std::iter::once("calipix")
        .chain(args.iter().copied())
        .map(OsString::from);
    calipix_app::cli::main(argv)
}

fn gradient(name: &str) -> PathBuf {
    let path = temp(name);
    let pixels: Vec<u8> = (0..=255).collect();
    calipix_io::save(&Image::from_vec(16, 16, pixels).unwrap(), &path).unwrap();
    path
}

#[test]
fn lists_and_describes_commands() {
    let listing = calipix(&["commands"]).unwrap();
    assert!(listing.contains("image.invert"), "{listing}");
    assert!(listing.contains("file.open"), "{listing}");

    let help = calipix(&["commands", "file.save_as"]).unwrap();
    assert!(help.contains("--input <image file, required>"), "{help}");
    assert!(help.contains("--path <path, required>"), "{help}");
    assert!(help.contains("Menu: File > Save As"), "{help}");

    let json: serde_json::Value =
        serde_json::from_str(&calipix(&["commands", "--json"]).unwrap()).unwrap();
    let ids: Vec<_> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"image.invert"));
    assert!(json[0]["input_schema"]["type"] == "object");
}

#[test]
fn runs_a_command_and_saves_the_result() {
    let source = gradient("run-source.png");
    let target = temp("run-inverted.png");
    let out = calipix(&[
        "run",
        "image.invert",
        "--input",
        source.to_str().unwrap(),
        "-o",
        target.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(out, "");

    let inverted = calipix_io::open(&target).unwrap().read_plane(0).unwrap();
    let expected: Vec<u8> = (0..=255).rev().collect();
    assert_eq!(inverted.as_slice::<u8>().unwrap(), expected);
}

#[test]
fn prints_outputs_as_text_or_json() {
    let source = gradient("info-source.png");
    let text = calipix(&["run", "image.info", "--input", source.to_str().unwrap()]).unwrap();
    assert!(text.contains("width: 16\n"), "{text}");
    assert!(text.contains("pixel_type: u8\n"), "{text}");
    assert!(text.contains("name: info-source.png\n"), "{text}");

    let equals_form = format!("--input={}", source.display());
    let json = calipix(&["run", "--json", "image.info", &equals_form]).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["height"], 16);
    assert_eq!(json["in_memory"], true);
}

#[test]
fn reports_mistakes_clearly() {
    let source = gradient("errors-source.png");
    let source = source.to_str().unwrap();
    let message = |args: &[&str]| format!("{:#}", calipix(args).unwrap_err());

    assert!(message(&["run", "image.nope"]).contains("unknown command `image.nope`"));
    let unknown = message(&["run", "image.invert", "--sigma", "2"]);
    assert!(unknown.contains("no parameter --sigma"), "{unknown}");
    assert!(unknown.contains("it takes: --input"), "{unknown}");
    assert!(message(&["run", "image.invert"]).contains("missing required parameter `input`"));
    assert!(message(&["run", "image.invert", "--input"]).contains("--input needs a value"));
    assert!(message(&["run", "image.invert", source]).contains("unexpected argument"));
    let missing = message(&["run", "image.invert", "--input", "no-such-file.png"]);
    assert!(missing.contains("--input"), "{missing}");
    assert!(missing.contains("no-such-file.png"), "{missing}");
    let unsaveable = message(&["run", "image.invert", "--input", source, "-o", "out.xyz"]);
    assert!(
        unsaveable.contains("unrecognised image format"),
        "{unsaveable}"
    );
    assert!(message(&["run", "image.invert", "--input", source, "-o"]).contains("-o needs"));
}

/// Commands that act on the interface are listed like any other, but
/// cannot be run without one.
#[test]
fn interface_commands_are_listed_but_not_run() {
    let listing = calipix(&["commands"]).unwrap();
    assert!(listing.contains("view.zoom_in"), "{listing}");
    assert!(listing.contains("help.command_palette"), "{listing}");
    let help = calipix(&["commands", "edit.undo"]).unwrap();
    assert!(help.contains("Menu: Edit > Undo"), "{help}");

    for id in ["view.zoom_in", "edit.undo", "window.toggle_info"] {
        let refusal = format!("{:#}", calipix(&["run", id]).unwrap_err());
        assert!(refusal.contains(id), "{refusal}");
        assert!(
            refusal
                .contains("acts on the desktop interface and cannot be run from the command line"),
            "{refusal}"
        );
    }
}

#[test]
fn help_and_version_are_not_errors() {
    assert!(calipix(&["--help"]).unwrap().contains("Usage: calipix"));
    assert!(calipix(&["--version"]).unwrap().starts_with("calipix "));
    assert!(
        calipix(&["run", "--help"])
            .unwrap()
            .contains("--name value")
    );
}

/// The real binary: exit codes and which stream output goes to.
#[test]
fn binary_exit_codes_and_streams() {
    let exe = env!("CARGO_BIN_EXE_calipix");
    let ok = Command::new(exe).args(["commands"]).output().unwrap();
    assert!(ok.status.success());
    assert!(String::from_utf8_lossy(&ok.stdout).contains("image.invert"));
    assert!(ok.stderr.is_empty());

    let bad = Command::new(exe)
        .args(["run", "image.nope"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(bad.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("error: unknown command"));
}

/// A display command from the command line: the lookup table it sets is
/// in the TIFF written, and the samples are the ones read.
#[test]
fn a_display_command_runs_headless_and_its_result_is_saved() {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/imagej/out/gray8.tif");
    let (a, b) = (temp("a.tif"), temp("b.tif"));
    std::fs::copy(&source, &a).unwrap();
    let output = calipix(&[
        "run",
        "view.set_lut",
        "--input",
        a.to_str().unwrap(),
        "--lut",
        "fire",
        "-o",
        b.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(output, "", "a display command reports nothing");

    let (before, after) = (calipix_io::open(&a).unwrap(), calipix_io::open(&b).unwrap());
    let fire = calipix_render::builtin_lut("fire").unwrap();
    assert_eq!(
        after.meta().channels[0].lut.as_ref().unwrap().table,
        fire.table
    );
    assert_eq!(after.read_plane(0).unwrap(), before.read_plane(0).unwrap());

    // Ranges too, and the plane they are taken from is named as in ImageJ.
    let stack = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out/hyperstack_c3z4t5.tif");
    let c = temp("c.tif");
    calipix(&[
        "run",
        "view.auto_contrast",
        "--input",
        stack.to_str().unwrap(),
        "--channel",
        "2",
        "--slice",
        "3",
        "--frame=4",
        "-o",
        c.to_str().unwrap(),
    ])
    .unwrap();
    let opened = calipix_io::open(&stack).unwrap();
    let expected = calipix_render::auto_contrast(&opened, &[1, 2, 3], 1).unwrap();
    let saved = calipix_io::open(&c).unwrap();
    assert_eq!(saved.meta().channels[1].display_range, Some(expected.range));
    assert_eq!(
        saved.meta().channels[0].display_range,
        opened.meta().channels[0].display_range
    );

    let help = calipix(&["commands", "view.set_lut"]).unwrap();
    assert!(help.contains("Menu: View > Set Lookup Table"), "{help}");
    assert!(help.contains("fire|grays|ice"), "{help}");
    let error = calipix(&[
        "run",
        "view.set_lut",
        "--input",
        a.to_str().unwrap(),
        "--lut",
        "plasma",
    ])
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("`plasma` is not one of"),
        "{error:#}"
    );
}
