use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use imager_command::{
    Args, CancelToken, CommandSpec, DatasetId, Effects, EffectsPolicy, Error, Executor,
    ImageEffect, Outputs, ParamType, Params, PathMode, Progress, Registry, RunOptions, Session,
    Value, conformance, typed,
};
use imager_core::{Buffer, Image};

#[derive(Params)]
struct NewParams {
    #[param(default = 4, min = 1, max = 64)]
    width: i64,
    #[param(default = 7)]
    fill: i64,
}

#[derive(Params)]
#[allow(dead_code)]
struct ScaleParams {
    input: DatasetId,
    /// Multiplier applied to every sample.
    #[param(label = "Factor", default = 2.0, min = 0, unit = "x", ijm = "value")]
    factor: f64,
    #[param(choices("clamp", "wrap"), default = "clamp", advanced)]
    overflow: String,
    note: Option<String>,
}

#[derive(Params)]
#[allow(dead_code)]
struct ExportParams {
    input: DatasetId,
    #[param(path = "save")]
    path: PathBuf,
}

#[derive(Params)]
struct InputOnly {
    input: DatasetId,
}

fn pixels(session: &Session, id: DatasetId) -> Vec<u8> {
    let image = session.workspace.image(id).unwrap();
    image
        .read_plane(0)
        .unwrap()
        .as_slice::<u8>()
        .unwrap()
        .to_vec()
}

fn registry() -> Registry {
    let mut reg = Registry::new();

    reg.register(typed(
        CommandSpec::new("test.new", "New")
            .summary("Creates a one-row image.")
            .menu("File", 0.0)
            .output("image", ParamType::Image)
            .effects(Effects {
                image: ImageEffect::Creates,
                ..Effects::default()
            }),
        |ctx, p: NewParams| {
            let image = Image::from_vec(p.width as usize, 1, vec![p.fill as u8; p.width as usize])?;
            let id = ctx.workspace_mut().insert(image, None);
            Ok(Outputs::new().with("image", id))
        },
    ))
    .unwrap();

    reg.register(typed(
        CommandSpec::new("test.scale", "Scale")
            .summary("Multiplies every sample.")
            .menu("Process > Math", 0.0)
            .ijm_name("Multiply...")
            .effects(Effects {
                image: ImageEffect::Modifies,
                ..Effects::default()
            }),
        |ctx, p: ScaleParams| {
            let image = ctx.image(p.input)?.map_planes(|_, b| {
                let s = b.try_slice::<u8>()?;
                Ok(Buffer::from_vec(
                    s.iter()
                        .map(|&v| {
                            let scaled = v as f64 * p.factor;
                            if p.overflow == "wrap" {
                                scaled as u64 as u8
                            } else {
                                scaled.min(255.0) as u8
                            }
                        })
                        .collect::<Vec<_>>(),
                ))
            })?;
            ctx.workspace_mut().set_image(p.input, image)?;
            Ok(Outputs::new())
        },
    ))
    .unwrap();

    reg.register(typed(
        CommandSpec::new("test.fail_midway", "Fail midway")
            .summary("Changes the workspace and then fails.")
            .effects(Effects {
                image: ImageEffect::Modifies,
                ..Effects::default()
            }),
        |ctx, p: InputOnly| {
            ctx.workspace_mut().remove(p.input)?;
            Err(Error::failed("deliberate"))
        },
    ))
    .unwrap();

    reg.register(typed(
        CommandSpec::new("test.count", "Count")
            .summary("Counts samples.")
            .output("count", ParamType::Int),
        |ctx, p: InputOnly| {
            let n = ctx.image(p.input)?.meta().plane_len();
            Ok(Outputs::new().with("count", n))
        },
    ))
    .unwrap();

    reg.register(typed(
        CommandSpec::new("test.export", "Export")
            .summary("Pretends to write a file.")
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        |_, _: ExportParams| Ok(Outputs::new()),
    ))
    .unwrap();

    reg.register(typed(
        CommandSpec::new("test.slow", "Slow")
            .summary("Reports progress and honours cancellation.")
            .output("steps", ParamType::Int),
        |ctx, _: InputOnly| {
            for step in 0..10 {
                ctx.progress(step as f64 / 10.0, "working");
                ctx.check_cancelled()?;
            }
            Ok(Outputs::new().with("steps", 10))
        },
    ))
    .unwrap();

    reg.register(typed(
        CommandSpec::new("test.bad_output", "Bad output")
            .summary("Returns something it did not declare.")
            .output("count", ParamType::Int),
        |_, _: InputOnly| Ok(Outputs::new().with("count", "three")),
    ))
    .unwrap();

    reg
}

fn setup() -> (Executor, Session) {
    (
        Executor::new(Arc::new(registry()), EffectsPolicy::allow_all()),
        Session::new(),
    )
}

#[test]
fn derive_builds_specs_from_fields() {
    let specs = ScaleParams::specs();
    let names: Vec<_> = specs.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["input", "factor", "overflow", "note"]);

    let factor = &specs[1];
    assert_eq!(factor.ty, ParamType::Float);
    assert_eq!(factor.label, "Factor");
    assert_eq!(factor.doc, "Multiplier applied to every sample.");
    assert_eq!(factor.default, Some(Value::Float(2.0)));
    assert_eq!(factor.min, Some(0.0));
    assert_eq!(factor.unit.as_deref(), Some("x"));
    assert_eq!(factor.ijm_key.as_deref(), Some("value"));

    let overflow = &specs[2];
    assert_eq!(
        overflow.ty,
        ParamType::Choice(vec!["clamp".into(), "wrap".into()])
    );
    assert_eq!(overflow.default, Some(Value::from("clamp")));
    assert!(overflow.advanced);

    assert!(specs[0].required);
    assert!(!specs[3].required);
    assert_eq!(ExportParams::specs()[1].ty, ParamType::Path(PathMode::Save));
}

#[test]
fn runs_with_defaults_and_active_image() {
    let (exec, mut session) = setup();
    let out = exec.run(&mut session, "test.new", &Args::new()).unwrap();
    let Some(Value::Image(id)) = out.get("image").cloned() else {
        panic!("no image output");
    };
    assert_eq!(pixels(&session, id), [7; 4]);

    // `input` falls back to the active image, `factor` to its default.
    exec.run(&mut session, "test.scale", &Args::new()).unwrap();
    assert_eq!(pixels(&session, id), [14; 4]);

    // Integer arguments are accepted for float parameters.
    exec.run(&mut session, "test.scale", &Args::new().with("factor", 100))
        .unwrap();
    assert_eq!(pixels(&session, id), [255; 4]);

    let entry = session.history().entries().last().unwrap();
    assert_eq!(entry.command, "test.scale");
    assert_eq!(entry.args.get("factor"), Some(&Value::Float(100.0)));
    assert_eq!(entry.args.get("input"), Some(&Value::Image(id)));
    assert_eq!(entry.args.get("overflow"), Some(&Value::from("clamp")));
    assert_eq!(entry.args.get("note"), None);
}

#[test]
fn rejects_bad_arguments() {
    let (exec, mut session) = setup();
    let run = |session: &mut Session, id: &str, args: Args| exec.run(session, id, &args);

    assert!(matches!(
        run(&mut session, "test.nope", Args::new()),
        Err(Error::UnknownCommand(_))
    ));
    // No image is open, so `input` cannot be resolved.
    assert!(matches!(
        run(&mut session, "test.scale", Args::new()),
        Err(Error::MissingArg { param, .. }) if param == "input"
    ));
    run(&mut session, "test.new", Args::new()).unwrap();
    assert!(matches!(
        run(&mut session, "test.scale", Args::new().with("sigma", 1)),
        Err(Error::UnknownArg { .. })
    ));
    assert!(matches!(
        run(&mut session, "test.scale", Args::new().with("factor", -1.0)),
        Err(Error::InvalidArg { .. })
    ));
    assert!(matches!(
        run(
            &mut session,
            "test.scale",
            Args::new().with("factor", "big")
        ),
        Err(Error::InvalidArg { .. })
    ));
    assert!(matches!(
        run(
            &mut session,
            "test.scale",
            Args::new().with("overflow", "explode")
        ),
        Err(Error::InvalidArg { .. })
    ));
    assert!(matches!(
        run(
            &mut session,
            "test.scale",
            Args::new().with("input", DatasetId(99))
        ),
        Err(Error::NoDataset(_))
    ));
    assert!(matches!(
        run(&mut session, "test.new", Args::new().with("width", 65)),
        Err(Error::InvalidArg { .. })
    ));
    // Only the successful `test.new` was recorded.
    assert_eq!(session.history().entries().len(), 1);
}

#[test]
fn failed_commands_leave_the_session_untouched() {
    let (exec, mut session) = setup();
    exec.run(&mut session, "test.new", &Args::new()).unwrap();
    let id = session.workspace.active().unwrap();

    let err = exec.run(&mut session, "test.fail_midway", &Args::new());
    assert!(matches!(err, Err(Error::Failed(_))));
    assert_eq!(session.workspace.len(), 1);
    assert_eq!(pixels(&session, id), [7; 4]);
    assert_eq!(session.history().entries().len(), 1);

    assert!(
        exec.run(&mut session, "test.bad_output", &Args::new())
            .is_err()
    );
    assert_eq!(session.history().entries().len(), 1);
}

#[test]
fn undo_and_redo_restore_exact_states() {
    let (exec, mut session) = setup();
    assert!(matches!(exec.undo(&mut session), Err(Error::NothingToUndo)));

    exec.run(&mut session, "test.new", &Args::new()).unwrap();
    let id = session.workspace.active().unwrap();
    let original = session.workspace.image(id).unwrap().clone();
    exec.run(&mut session, "test.scale", &Args::new()).unwrap();
    exec.run(&mut session, "test.count", &Args::new()).unwrap();
    exec.run(&mut session, "test.scale", &Args::new()).unwrap();
    assert_eq!(pixels(&session, id), [28; 4]);

    assert_eq!(exec.undo(&mut session).unwrap(), "Scale");
    assert_eq!(pixels(&session, id), [14; 4]);
    // Undo steps over `test.count`, which changed nothing.
    assert_eq!(exec.undo(&mut session).unwrap(), "Scale");
    assert_eq!(pixels(&session, id), [7; 4]);
    // The restored image is the very same snapshot, not a recomputation.
    assert!(session.workspace.image(id).unwrap().ptr_eq(&original));

    assert_eq!(exec.undo(&mut session).unwrap(), "New");
    assert!(session.workspace.is_empty());
    assert_eq!(session.workspace.active(), None);
    assert!(!session.history().can_undo());
    assert!(matches!(exec.undo(&mut session), Err(Error::NothingToUndo)));

    assert_eq!(exec.redo(&mut session).unwrap(), "New");
    assert_eq!(session.workspace.active(), Some(id));
    assert_eq!(exec.redo(&mut session).unwrap(), "Scale");
    assert_eq!(exec.redo(&mut session).unwrap(), "Scale");
    assert_eq!(pixels(&session, id), [28; 4]);
    assert!(matches!(exec.redo(&mut session), Err(Error::NothingToRedo)));

    // A new command after an undo discards the redo tail.
    exec.undo(&mut session).unwrap();
    exec.run(&mut session, "test.scale", &Args::new().with("factor", 3))
        .unwrap();
    assert_eq!(pixels(&session, id), [42; 4]);
    assert!(!session.history().can_redo());
    let applied: Vec<_> = session
        .history()
        .applied()
        .iter()
        .map(|e| e.command.as_str())
        .collect();
    // `test.count` ran between the two scales and is still part of the record.
    assert_eq!(
        applied,
        ["test.new", "test.scale", "test.count", "test.scale"]
    );
}

#[test]
fn policy_gates_effects() {
    let (mut exec, mut session) = setup();
    exec.run(&mut session, "test.new", &Args::new()).unwrap();
    let args = Args::new().with("path", "out.tif");

    exec.policy = EffectsPolicy::read_only();
    assert!(matches!(
        exec.run(&mut session, "test.export", &args),
        Err(Error::Denied(_))
    ));
    exec.policy = EffectsPolicy::allow_all();
    exec.run(&mut session, "test.export", &args).unwrap();
}

#[test]
fn progress_and_cancellation() {
    struct Collect(Mutex<Vec<f64>>);
    impl Progress for Collect {
        fn report(&self, fraction: f64, _message: &str) {
            self.0.lock().unwrap().push(fraction);
        }
    }

    let (exec, mut session) = setup();
    exec.run(&mut session, "test.new", &Args::new()).unwrap();

    let progress = Collect(Mutex::new(Vec::new()));
    let options = RunOptions {
        progress: &progress,
        cancel: CancelToken::new(),
    };
    exec.run_with(&mut session, "test.slow", &Args::new(), &options)
        .unwrap();
    assert_eq!(progress.0.lock().unwrap().len(), 10);

    let options = RunOptions::default();
    options.cancel.cancel();
    assert!(matches!(
        exec.run_with(&mut session, "test.slow", &Args::new(), &options),
        Err(Error::Cancelled)
    ));
}

#[test]
fn unresolved_lists_what_a_dialog_must_ask_for() {
    let (exec, mut session) = setup();
    let spec = exec.registry().get("test.export").unwrap().spec().clone();
    assert_eq!(
        exec.unresolved(&spec, &session.workspace, &Args::new()),
        ["input", "path"]
    );
    exec.run(&mut session, "test.new", &Args::new()).unwrap();
    assert_eq!(
        exec.unresolved(&spec, &session.workspace, &Args::new()),
        ["path"]
    );
    assert!(
        exec.unresolved(
            &spec,
            &session.workspace,
            &Args::new().with("path", "a.tif")
        )
        .is_empty()
    );
}

#[test]
fn registry_rejects_duplicates_and_finds_ijm_names() {
    let mut reg = registry();
    let dup = typed(
        CommandSpec::new("test.count", "Count again").summary("Duplicate."),
        |_, _: InputOnly| Ok(Outputs::new()),
    );
    assert!(matches!(reg.register(dup), Err(Error::DuplicateCommand(_))));
    let invalid = typed(CommandSpec::new("nodots", "Bad"), |_, _: InputOnly| {
        Ok(Outputs::new())
    });
    assert!(matches!(
        reg.register(invalid),
        Err(Error::InvalidSpec { .. })
    ));
    assert_eq!(
        reg.by_ijm_name("Multiply...").unwrap().spec().id,
        "test.scale"
    );
    assert!(reg.by_ijm_name("Nope").is_none());
}

#[test]
fn conformance_reports_problems() {
    assert_eq!(conformance::check(&registry()), Vec::<String>::new());

    let mut reg = registry();
    reg.register(typed(
        CommandSpec::new("test.scale_again", "Scale")
            .menu("Process > Math", 1.0)
            .ijm_name("Multiply..."),
        |_, _: InputOnly| Ok(Outputs::new()),
    ))
    .unwrap();
    let problems = conformance::check(&reg);
    assert_eq!(problems.len(), 3, "{problems:#?}");
    assert!(problems.iter().any(|p| p.contains("summary is empty")));
    assert!(problems.iter().any(|p| p.contains("ImageJ name")));
    assert!(problems.iter().any(|p| p.contains("menu item")));
}

#[test]
fn specs_export_as_json() {
    let reg = registry();
    let json = reg.get("test.scale").unwrap().spec().to_json();
    assert_eq!(json["id"], "test.scale");
    assert_eq!(json["ijm_name"], "Multiply...");
    assert_eq!(json["effects"]["image"], "modifies");
    assert_eq!(json["inputs"][2]["choices"][1], "wrap");
    assert_eq!(json["input_schema"]["required"][0], "input");
    assert_eq!(
        json["input_schema"]["properties"]["overflow"]["enum"][0],
        "clamp"
    );
}
