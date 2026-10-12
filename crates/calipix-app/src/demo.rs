//! Synthetic commands that exercise every part of the interface: one with
//! a parameter of each type, and one that takes a while. They are not part
//! of the application; tests register them, and setting
//! `CALIPIX_DEMO_COMMANDS` adds them to the desktop interface for trying
//! out dialogs and progress reporting by hand.

use std::sync::Arc;
use std::time::{Duration, Instant};

use calipix_core::{Buffer, DType, Image, ImageMeta};

use calipix_command::{
    Args, Command, CommandCtx, CommandSpec, Effects, Flags, Outputs, ParamSpec, ParamType,
    PathMode, Registry, Result, Value,
};

struct Demo<F> {
    spec: CommandSpec,
    run: F,
}

impl<F> Command for Demo<F>
where
    F: Fn(&mut CommandCtx<'_>, &Args) -> Result<Outputs> + Send + Sync,
{
    fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    fn run(&self, ctx: &mut CommandCtx<'_>, args: &Args) -> Result<Outputs> {
        (self.run)(ctx, args)
    }
}

/// `demo.parameters`: takes one parameter of every type and reports them
/// back as text.
pub fn parameters() -> Arc<dyn Command> {
    let choices = ["Nearest", "Bilinear", "Bicubic"]
        .map(String::from)
        .to_vec();
    let spec = CommandSpec::new("demo.parameters", "Parameter Gallery")
        .summary("Takes one parameter of every type and reports what it was given.")
        .menu("Demo", 10.0)
        .input(ParamSpec::new("input", ParamType::Image).label("Image"))
        .input(
            ParamSpec::new("radius", ParamType::Float)
                .doc("How far the effect reaches.")
                .default_value(2.0)
                .min(0.0)
                .step(0.1)
                .unit("pixels"),
        )
        .input(
            ParamSpec::new("count", ParamType::Int)
                .doc("How many times to repeat.")
                .default_value(3)
                .min(1.0)
                .max(10.0),
        )
        .input(
            ParamSpec::new("normalize", ParamType::Bool)
                .label("Normalize result")
                .default_value(true),
        )
        .input(
            ParamSpec::new("title", ParamType::Str)
                .label("Title")
                .default_value("Result"),
        )
        .input(
            ParamSpec::new("method", ParamType::Choice(choices))
                .label("Interpolation")
                .default_value("Bilinear"),
        )
        .input(
            ParamSpec::new("report", ParamType::Path(PathMode::Save))
                .label("Report file")
                .required(false)
                .persist(false),
        )
        .input(
            ParamSpec::new("weights", ParamType::List(Box::new(ParamType::Float))).default_value(
                Value::List(vec![
                    Value::Float(1.0),
                    Value::Float(2.0),
                    Value::Float(1.0),
                ]),
            ),
        )
        .input(
            ParamSpec::new("mask", ParamType::Image)
                .doc("Restricts the effect to part of the image.")
                .required(false)
                .advanced(true),
        )
        .input(
            ParamSpec::new("iterations", ParamType::Int)
                .default_value(1)
                .min(1.0)
                .advanced(true),
        )
        .output("given", ParamType::Str)
        .effects(Effects {
            writes_fs: true,
            ..Effects::default()
        });
    Arc::new(Demo {
        spec,
        run: |_: &mut CommandCtx<'_>, args: &Args| {
            let given: Vec<String> = args
                .iter()
                .map(|(name, value)| format!("{name}={}", value.to_text()))
                .collect();
            Ok(Outputs::new().with("given", given.join(" ")))
        },
    })
}

/// `demo.slow`: does nothing for a number of seconds, reporting progress
/// and stopping early if cancelled.
pub fn slow() -> Arc<dyn Command> {
    let spec = CommandSpec::new("demo.slow", "Slow Command")
        .summary("Takes a while, to show progress reporting and cancelling.")
        .menu("Demo", 20.0)
        .input(
            ParamSpec::new("seconds", ParamType::Float)
                .default_value(3.0)
                .min(0.0)
                .max(60.0)
                .unit("s"),
        )
        .flags(Flags {
            cancellable: true,
            ..Flags::default()
        });
    Arc::new(Demo {
        spec,
        run: |ctx: &mut CommandCtx<'_>, args: &Args| {
            let Some(Value::Float(seconds)) = args.get("seconds") else {
                unreachable!("the executor checked the arguments");
            };
            let total = Duration::from_secs_f64(*seconds);
            let started = Instant::now();
            while started.elapsed() < total {
                ctx.check_cancelled()?;
                let fraction = started.elapsed().as_secs_f64() / total.as_secs_f64();
                ctx.progress(fraction, "Waiting");
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(Outputs::new())
        },
    })
}

/// `demo.test_stack`: makes a stack of 8-bit planes, a ramp with noise on
/// it, big enough for filters to take a while on.
pub fn test_stack() -> Arc<dyn Command> {
    let size = |name: &str, default: i64, max: f64| {
        ParamSpec::new(name, ParamType::Int)
            .default_value(default)
            .min(1.0)
            .max(max)
    };
    let spec = CommandSpec::new("demo.test_stack", "Test Stack")
        .summary("Creates a stack of noisy 8-bit planes, for trying slow commands on.")
        .menu("Demo", 30.0)
        .input(size("width", 1024, 8192.0).unit("pixels"))
        .input(size("height", 1024, 8192.0).unit("pixels"))
        .input(size("slices", 20, 1000.0))
        .output("output", ParamType::Image)
        .effects(Effects {
            image: calipix_command::ImageEffect::Creates,
            ..Effects::default()
        });
    Arc::new(Demo {
        spec,
        run: |ctx: &mut CommandCtx<'_>, args: &Args| {
            let int = |name: &str| match args.get(name) {
                Some(Value::Int(value)) => *value as usize,
                _ => unreachable!("the executor checked the arguments"),
            };
            let (width, height, slices) = (int("width"), int("height"), int("slices"));
            let mut meta = ImageMeta::xyczt(DType::U8, width, height, 1, slices, 1)?;
            meta.name = "Test Stack".to_string();
            // A small generator, so that the stack is the same every time.
            let mut seed = 0x2545_f491_4f6c_dd1d_u64;
            let planes = (0..slices)
                .map(|slice| {
                    let samples: Vec<u8> = (0..width * height)
                        .map(|index| {
                            seed ^= seed << 13;
                            seed ^= seed >> 7;
                            seed ^= seed << 17;
                            let ramp = (index % width) * 160 / width + slice * 2;
                            (ramp as u64 + seed % 64).min(255) as u8
                        })
                        .collect();
                    Buffer::from_vec(samples)
                })
                .collect();
            let id = ctx
                .workspace_mut()
                .insert(Image::from_planes(meta, planes)?, None);
            Ok(Outputs::new().with("output", id))
        },
    })
}

/// Adds the demo commands to `registry`.
pub fn register(registry: &mut Registry) -> Result<()> {
    registry.register(parameters())?;
    registry.register(slow())?;
    registry.register(test_stack())
}
