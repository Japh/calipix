//! Command-line interface.
//!
//! `imager2 run <id>` exposes every registered command. Its options are not
//! declared here: they are derived from the command's spec, so a command
//! added to the registry is immediately usable from the shell.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use imager_command::{
    Args, CommandSpec, EffectsPolicy, Executor, ParamType, Registry, Session, Value,
};

#[derive(Parser)]
#[command(
    name = "imager2",
    version,
    about = "Scientific image processing",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Image files to open
    files: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a command without the user interface
    ///
    /// Parameters are given as --name value. For image parameters, give
    /// the path of an image file. Use -o to save the resulting image.
    /// See `imager2 commands <ID>` for a command's parameters.
    Run {
        /// Print the command's outputs as JSON
        #[arg(long)]
        json: bool,

        /// Command id, e.g. image.invert
        id: String,

        /// Command parameters, then optionally -o <FILE>
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List the available commands, or describe one
    Commands {
        /// Print machine-readable descriptions
        #[arg(long)]
        json: bool,

        /// Show the parameters of this command
        id: Option<String>,
    },
}

/// Runs the application for the given command line and returns what should
/// be printed to standard output.
pub fn main(argv: impl IntoIterator<Item = OsString>) -> Result<String> {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        // --help and --version arrive as "errors" carrying the text to show.
        Err(e) if !e.use_stderr() => return Ok(e.to_string()),
        Err(e) => bail!("{}", e.to_string().trim_start_matches("error: ").trim_end()),
    };
    // Application commands are listed too, so that every frontend shows
    // the same set; `run` explains why it cannot run them.
    let registry = Arc::new(crate::state::action::full_registry());
    match cli.command {
        Some(Cmd::Run { json, id, args }) => run_command(registry, &id, &args, json),
        Some(Cmd::Commands { json, id }) => list_commands(&registry, id.as_deref(), json),
        None => open_interface(cli.files),
    }
}

#[cfg(feature = "gui")]
fn open_interface(files: Vec<PathBuf>) -> Result<String> {
    crate::gui::run(files)?;
    Ok(String::new())
}

#[cfg(not(feature = "gui"))]
fn open_interface(_files: Vec<PathBuf>) -> Result<String> {
    bail!("this is a headless build without the desktop interface; see `imager2 --help`")
}

fn run_command(registry: Arc<Registry>, id: &str, tokens: &[String], json: bool) -> Result<String> {
    let executor = Executor::new(registry, EffectsPolicy::allow_all());
    let spec = executor.registry().get(id)?.spec().clone();
    if !spec.flags.headless_ok {
        bail!(
            "`{id}` ({}) acts on the desktop interface and cannot be run from the command line",
            spec.title
        );
    }
    let RunTokens { params, save_to } = split_tokens(&spec, tokens)?;

    let mut session = Session::new();
    let mut args = Args::new();
    for (name, text) in params {
        let param = spec
            .param(&name)
            .expect("split_tokens only returns known names");
        let value = if param.ty == ParamType::Image {
            // On the command line an image is named by the file it is in.
            let opened = executor
                .run(
                    &mut session,
                    "file.open",
                    &Args::new().with("path", text.as_str()),
                )
                .with_context(|| format!("--{name}"))?;
            opened
                .get("image")
                .cloned()
                .ok_or_else(|| anyhow!("file.open returned no image"))?
        } else {
            Value::parse_text(&param.ty, &text).map_err(|e| anyhow!("--{name}: {e}"))?
        };
        args.insert(name, value);
    }

    let outputs = executor.run(&mut session, id, &args)?;

    if let Some(path) = save_to {
        let target = outputs
            .iter()
            .find_map(|(_, v)| match v {
                Value::Image(id) => Some(*id),
                _ => None,
            })
            .or(session.workspace.active())
            .ok_or_else(|| anyhow!("-o was given but `{id}` left no image to save"))?;
        executor.run(
            &mut session,
            "file.save_as",
            &Args::new().with("input", target).with("path", path),
        )?;
    }

    if json {
        return Ok(format!("{:#}\n", outputs.to_json()));
    }
    let mut text = String::new();
    for (name, value) in outputs.iter() {
        writeln!(text, "{name}: {}", value.to_text())?;
    }
    Ok(text)
}

/// What followed the command id on the command line.
struct RunTokens {
    /// Parameter names (as in the spec) with their unparsed values.
    params: Vec<(String, String)>,
    /// The `-o <FILE>` save target, if given.
    save_to: Option<PathBuf>,
}

/// Splits `--name value` tokens into parameter texts, checking names
/// against the spec, and pulls out the `-o <FILE>` save target.
fn split_tokens(spec: &CommandSpec, tokens: &[String]) -> Result<RunTokens> {
    let mut params = Vec::new();
    let mut save_to = None;
    let mut rest = tokens.iter().peekable();
    while let Some(token) = rest.next() {
        if token == "-o" {
            let path = rest.next().ok_or_else(|| anyhow!("-o needs a file path"))?;
            save_to = Some(PathBuf::from(path));
            continue;
        }
        let Some(body) = token.strip_prefix("--") else {
            bail!("unexpected argument `{token}`; parameters are given as --name value");
        };
        let (name, inline) = match body.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (body, None),
        };
        let name = name.replace('-', "_");
        let Some(param) = spec.param(&name) else {
            let known: Vec<String> = spec
                .inputs
                .iter()
                .map(|p| format!("--{}", p.name.replace('_', "-")))
                .collect();
            bail!(
                "`{}` has no parameter --{}; it takes: {}",
                spec.id,
                name.replace('_', "-"),
                if known.is_empty() {
                    "nothing".to_string()
                } else {
                    known.join(", ")
                }
            );
        };
        let value = match inline {
            Some(value) => value,
            None => {
                let next_is_value = rest
                    .peek()
                    .is_some_and(|t| !t.starts_with("--") && *t != "-o");
                if param.ty == ParamType::Bool && !next_is_value {
                    // A bare flag switches a boolean on.
                    "true".to_string()
                } else {
                    rest.next()
                        .ok_or_else(|| anyhow!("--{} needs a value", name.replace('_', "-")))?
                        .clone()
                }
            }
        };
        params.push((name, value));
    }
    Ok(RunTokens { params, save_to })
}

fn list_commands(registry: &Registry, id: Option<&str>, json: bool) -> Result<String> {
    if let Some(id) = id {
        let spec = registry.get(id)?.spec();
        return Ok(if json {
            format!("{:#}\n", spec.to_json())
        } else {
            describe(spec)
        });
    }
    if json {
        let all: Vec<_> = registry.specs().map(CommandSpec::to_json).collect();
        return Ok(format!("{:#}\n", serde_json::Value::Array(all)));
    }
    let width = registry.specs().map(|s| s.id.len()).max().unwrap_or(0);
    let mut text = String::new();
    for spec in registry.specs() {
        writeln!(text, "{:width$}  {}", spec.id, spec.summary)?;
    }
    Ok(text)
}

fn describe(spec: &CommandSpec) -> String {
    let mut text = format!("{} - {}\n{}\n", spec.id, spec.title, spec.summary);
    if !spec.doc.is_empty() {
        text.push_str(&format!("\n{}\n", spec.doc));
    }
    if let Some(menu) = &spec.menu {
        text.push_str(&format!(
            "\nMenu: {} > {}\n",
            menu.path.join(" > "),
            spec.title
        ));
    }
    if !spec.inputs.is_empty() {
        text.push_str("\nParameters:\n");
    }
    for param in &spec.inputs {
        let mut notes = vec![match &param.ty {
            ParamType::Choice(choices) => choices.join("|"),
            ParamType::Image => "image file".to_string(),
            other => other.name(),
        }];
        if let Some(default) = &param.default {
            notes.push(format!("default {}", default.to_text()));
        } else if param.required {
            notes.push("required".to_string());
        }
        if let Some(unit) = &param.unit {
            notes.push(unit.clone());
        }
        text.push_str(&format!(
            "  --{} <{}>  {}\n",
            param.name.replace('_', "-"),
            notes.join(", "),
            if param.doc.is_empty() {
                &param.label
            } else {
                &param.doc
            }
        ));
    }
    if !spec.outputs.is_empty() {
        let names: Vec<_> = spec.outputs.iter().map(|o| o.name.as_str()).collect();
        text.push_str(&format!("\nOutputs: {}\n", names.join(", ")));
    }
    text
}
