use std::path::PathBuf;
use std::sync::Arc;

use imager_command::{Args, EffectsPolicy, Error, Executor, Session, Value, conformance};
use imager_commands::standard_registry;
use imager_core::Image;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imager-commands-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

/// Every built-in command must be usable from every frontend.
#[test]
fn standard_registry_is_conformant() {
    let registry = standard_registry();
    assert!(!registry.is_empty());
    assert_eq!(conformance::check(&registry), Vec::<String>::new());
}

/// Commands that can run headless must do so given only their required
/// arguments, with no user interaction.
#[test]
fn every_command_declares_headless_support_truthfully() {
    for spec in standard_registry().specs() {
        assert!(spec.flags.headless_ok, "{} is not headless", spec.id);
    }
}

#[test]
fn open_invert_save_round_trip() {
    let (exec, mut session) = setup();
    let source = temp("source.png");
    let pixels: Vec<u8> = (0..=255).collect();
    imager_io::save(&Image::from_vec(16, 16, pixels.clone()).unwrap(), &source).unwrap();

    let out = exec
        .run(
            &mut session,
            "file.open",
            &Args::new().with("path", source.clone()),
        )
        .unwrap();
    let Some(&Value::Image(id)) = out.get("image") else {
        panic!("file.open returned no image");
    };
    assert_eq!(
        session.workspace.get(id).unwrap().path.as_deref(),
        Some(&*source)
    );

    let info = exec.run(&mut session, "image.info", &Args::new()).unwrap();
    assert_eq!(info.get("name"), Some(&Value::from("source.png")));
    assert_eq!(info.get("width"), Some(&Value::Int(16)));
    assert_eq!(info.get("pixel_type"), Some(&Value::from("u8")));
    assert_eq!(info.get("planes"), Some(&Value::Int(1)));

    exec.run(&mut session, "image.invert", &Args::new())
        .unwrap();
    let saved = temp("inverted.png");
    exec.run(
        &mut session,
        "file.save_as",
        &Args::new().with("path", saved.clone()),
    )
    .unwrap();
    assert_eq!(
        session.workspace.get(id).unwrap().path.as_deref(),
        Some(&*saved)
    );

    let reopened = imager_io::open(&saved).unwrap();
    let inverted: Vec<u8> = pixels.iter().map(|v| 255 - v).collect();
    assert_eq!(
        reopened.read_plane(0).unwrap().as_slice::<u8>().unwrap(),
        inverted
    );

    // Undo restores the pixels; the file on disk is not touched.
    assert_eq!(exec.undo(&mut session).unwrap(), "Invert");
    let plane = session.workspace.image(id).unwrap().read_plane(0).unwrap();
    assert_eq!(plane.as_slice::<u8>().unwrap(), pixels);

    exec.run(&mut session, "file.close", &Args::new()).unwrap();
    assert!(session.workspace.is_empty());
}

#[test]
fn failures_are_reported_and_change_nothing() {
    let (exec, mut session) = setup();
    let missing = Args::new().with("path", temp("does-not-exist.png"));
    let err = exec.run(&mut session, "file.open", &missing).unwrap_err();
    assert!(matches!(err, Error::Failed(_)), "{err}");
    assert!(session.workspace.is_empty());
    assert!(session.history().entries().is_empty());

    assert!(matches!(
        exec.run(&mut session, "image.invert", &Args::new()),
        Err(Error::MissingArg { .. })
    ));
}

#[test]
fn read_only_policy_blocks_saving_but_not_opening() {
    let (mut exec, mut session) = setup();
    let source = temp("policy.png");
    imager_io::save(&Image::from_vec(1, 1, vec![1u8]).unwrap(), &source).unwrap();
    exec.policy = EffectsPolicy::read_only();

    exec.run(&mut session, "file.open", &Args::new().with("path", source))
        .unwrap();
    exec.run(&mut session, "image.invert", &Args::new())
        .unwrap();
    let target = temp("blocked.png");
    let err = exec
        .run(
            &mut session,
            "file.save_as",
            &Args::new().with("path", target.clone()),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err}");
    assert!(!target.exists());
}
