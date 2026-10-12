//! Unattended capture of the window, for checking the interface without a
//! person at the screen.
//!
//! Setting `CALIPIX_CAPTURE` to a PNG path makes the application carry out
//! the steps in `CALIPIX_CAPTURE_STEPS`, save a picture of its window there
//! and exit. Steps are separated by `;`:
//!
//! - `open:<path>` opens an image file
//! - `run:<command id>` chooses a command as from a menu
//! - `start:<command id>|<parameter>=<value>|...` runs a command with
//!   arguments, as the command line would
//! - `palette:<query>` opens the command palette and types a query
//! - `set:<parameter>=<value>` types a value into the open dialog
//! - `more` unfolds the dialog's advanced options
//! - `accept` presses OK in the open dialog
//! - `select:<n>` chooses the n-th image tab, counting from 1
//! - `position:<i>,<j>,...` moves the active image's view to a plane, one
//!   index per axis beyond X and Y, counting from 0
//! - `hide:<n>` and `show:<n>` hide or show channel n, counting from 1
//! - `drag:<x1>,<y1>,<x2>,<y2>` presses at one position and releases at
//!   another, as when dragging a range marker
//! - `panel:<name>` brings a panel to the front: `results`, `plot`, `log`...
//! - `table:<name>` and `plot:<name>` choose what the Results and Plot
//!   panels show
//! - `rows:<first>-<last>` selects rows of the table on show, from 1
//! - `log` prints the history, the status and what the viewer reports to
//!   standard error, to check a script did what it meant to
//! - `click:<x>,<y>` clicks at a position in the window, e.g. on a menu
//! - `double:<x>,<y>` double-clicks there
//! - `hover:<x>,<y>` moves the pointer there
//! - `wait:<milliseconds>` lets time pass
//! - `tool:<name>` chooses a tool: `pointer`, `rectangle`, `oval`,
//!   `polygon`, `freehand`, `line`, `segmented_line`, `point`
//! - `key:<shortcut>` presses a key, written as shortcuts are: `Escape`,
//!   `T`, `Alt+Right`, `Ctrl+Z`
//! - `mods:<modifiers>` holds `shift`, `alt` or `ctrl` (joined by `+`)
//!   down for the pointer steps that follow; `mods:none` lets go
//! - `idrag:<x1>,<y1>,<x2>,<y2>`, `iclick:<x>,<y>`, `ihover:<x>,<y>`,
//!   `ipress:<x>,<y>` and `irelease:<x>,<y>` are pointer steps at places
//!   given in the active image's own coordinates, wherever it is on screen
//! - `ipath:<x>,<y>,<x>,<y>,...` presses at the first place, moves through
//!   the others with the button down and releases at the last
//! - `wheel:<x>,<y>,<notches>` and `iwheel:<x>,<y>,<notches>` turn the
//!   mouse wheel over a place; positive notches zoom in
//! - `regions:<n>` gives the active image n generated regions
//! - `pick:<first>-<last>` selects regions in the Regions panel, from 1
//!
//! `CALIPIX_CAPTURE_RUN=<command id>` is short for a single `run` step. The
//! user's settings are neither read nor written while capturing, unless
//! `CALIPIX_CONFIG_DIR` names a folder to keep settings in.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use calipix_command::Args;
use calipix_core::{Buffer, ColorModel, DType, Image, ImageMeta};
use eframe::egui;

use super::Shell;
use super::layout::Pane;
use crate::state::shortcut::Shortcut;
use crate::state::{Draft, Panel, RowClick, Tool};

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Open(PathBuf),
    Run(String),
    Start(String, Vec<(String, String)>),
    Palette(String),
    Set(String, String),
    More,
    Accept,
    Select(usize),
    Position(Vec<usize>),
    Visible(usize, bool),
    Drag(egui::Pos2, egui::Pos2),
    Panel(String),
    Table(String),
    Plot(String),
    Rows(usize, usize),
    Log,
    Click(egui::Pos2),
    Double(egui::Pos2),
    Hover(egui::Pos2),
    Wait(Duration),
    Tool(Tool),
    Key(Shortcut),
    Mods(egui::Modifiers),
    /// A pointer step whose places are in image coordinates.
    OnImage(Box<Step>),
    Press(egui::Pos2),
    Release(egui::Pos2),
    /// Press, move through the places, release at the last.
    Path(Vec<egui::Pos2>),
    Wheel(egui::Pos2, f32),
    Regions(usize),
    Pick(usize, usize),
}

fn position(text: &str) -> Result<egui::Pos2> {
    let (x, y) = text
        .split_once(',')
        .ok_or_else(|| anyhow!("`{text}` should be <x>,<y>"))?;
    Ok(egui::pos2(x.trim().parse()?, y.trim().parse()?))
}

impl Step {
    fn parse(text: &str) -> Result<Step> {
        let (verb, rest) = text.split_once(':').unwrap_or((text, ""));
        Ok(match verb.trim() {
            "open" => Step::Open(rest.into()),
            "run" => Step::Run(rest.to_string()),
            "start" => {
                let mut parts = rest.split('|');
                let id = parts.next().unwrap_or_default().trim().to_string();
                let args = parts
                    .map(|part| {
                        let (name, value) = part
                            .split_once('=')
                            .ok_or_else(|| anyhow!("`{part}` should be <parameter>=<value>"))?;
                        Ok((name.trim().to_string(), value.to_string()))
                    })
                    .collect::<Result<_>>()?;
                Step::Start(id, args)
            }
            "palette" => Step::Palette(rest.to_string()),
            "set" => {
                let (name, value) = rest
                    .split_once('=')
                    .ok_or_else(|| anyhow!("`{text}` should be set:<parameter>=<value>"))?;
                Step::Set(name.to_string(), value.to_string())
            }
            "more" => Step::More,
            "accept" => Step::Accept,
            "select" => Step::Select(rest.parse().context("select needs a tab number")?),
            "position" => Step::Position(
                rest.split(',')
                    .map(|index| index.trim().parse())
                    .collect::<Result<_, _>>()
                    .context("position needs indices separated by commas")?,
            ),
            "hide" | "show" => Step::Visible(
                rest.parse()
                    .context("hide and show need a channel number")?,
                verb.trim() == "show",
            ),
            "drag" => {
                let numbers: Vec<&str> = rest.split(',').collect();
                let [x1, y1, x2, y2] = numbers.as_slice() else {
                    bail!("`{text}` should be drag:<x1>,<y1>,<x2>,<y2>");
                };
                Step::Drag(
                    position(&format!("{x1},{y1}"))?,
                    position(&format!("{x2},{y2}"))?,
                )
            }
            "panel" => Step::Panel(rest.trim().to_string()),
            "table" => Step::Table(rest.to_string()),
            "plot" => Step::Plot(rest.to_string()),
            "rows" => {
                let (first, last) = rest.split_once('-').unwrap_or((rest, rest));
                let number = |text: &str| text.trim().parse::<usize>();
                Step::Rows(
                    number(first).context("rows needs row numbers")?,
                    number(last).context("rows needs row numbers")?,
                )
            }
            "log" => Step::Log,
            "click" => Step::Click(position(rest)?),
            "double" => Step::Double(position(rest)?),
            "hover" => Step::Hover(position(rest)?),
            "wait" => Step::Wait(Duration::from_millis(
                rest.parse().context("wait needs milliseconds")?,
            )),
            "tool" => Step::Tool(
                Tool::from_key(rest.trim()).ok_or_else(|| anyhow!("`{rest}` is not a tool"))?,
            ),
            "key" => Step::Key(Shortcut::parse(rest.trim()).map_err(|e| anyhow!(e))?),
            "mods" => {
                let mut modifiers = egui::Modifiers::NONE;
                for name in rest.split('+').map(str::trim) {
                    match name {
                        "shift" => modifiers.shift = true,
                        "alt" => modifiers.alt = true,
                        "ctrl" => {
                            modifiers.ctrl = true;
                            modifiers.command = true;
                        }
                        "none" | "" => {}
                        other => bail!("`{other}` is not a modifier"),
                    }
                }
                Step::Mods(modifiers)
            }
            "press" => Step::Press(position(rest)?),
            "release" => Step::Release(position(rest)?),
            "wheel" | "iwheel" => {
                let numbers: Vec<&str> = rest.split(',').collect();
                let [x, y, notches] = numbers.as_slice() else {
                    bail!("`{text}` should be {verb}:<x>,<y>,<notches>");
                };
                let step = Step::Wheel(
                    position(&format!("{x},{y}"))?,
                    notches.trim().parse().context("wheel needs notches")?,
                );
                if verb.trim() == "iwheel" {
                    Step::OnImage(Box::new(step))
                } else {
                    step
                }
            }
            "ipath" => {
                let numbers: Vec<f32> = rest
                    .split(',')
                    .map(|n| n.trim().parse())
                    .collect::<Result<_, _>>()
                    .context("ipath needs numbers separated by commas")?;
                if numbers.len() < 4 || !numbers.len().is_multiple_of(2) {
                    bail!("`{text}` should be ipath:<x>,<y>,<x>,<y>,...");
                }
                let places = numbers.chunks(2).map(|p| egui::pos2(p[0], p[1])).collect();
                Step::OnImage(Box::new(Step::Path(places)))
            }
            "idrag" | "iclick" | "ihover" | "ipress" | "irelease" | "idouble" => {
                let plain = format!("{}:{rest}", &verb.trim()[1..]);
                Step::OnImage(Box::new(Step::parse(&plain)?))
            }
            "regions" => Step::Regions(rest.trim().parse().context("regions needs a count")?),
            "pick" => {
                let (first, last) = rest.split_once('-').unwrap_or((rest, rest));
                let number = |text: &str| text.trim().parse::<usize>();
                Step::Pick(
                    number(first).context("pick needs region numbers")?,
                    number(last).context("pick needs region numbers")?,
                )
            }
            other => bail!("unknown capture step `{other}`"),
        })
    }
}

pub struct Capture {
    path: PathBuf,
    steps: VecDeque<Step>,
    /// Frames drawn since the last step, so each step's effect is on
    /// screen before the next one or the picture.
    settled: u32,
    hold_until: Option<Instant>,
    /// Set by a `wait`, which is how a script looks at a command in
    /// mid-run: what follows does not wait for the command to finish.
    mid_run: bool,
    requested: bool,
    /// Pointer events to feed in, one batch per frame.
    pointer: VecDeque<Vec<egui::Event>>,
    /// Modifier keys held down for the pointer steps.
    held: egui::Modifiers,
}

/// Frames to let the interface settle after each step.
const SETTLE: u32 = 12;

impl Capture {
    pub fn from_env() -> Result<Option<Capture>> {
        let Some(path) = std::env::var_os("CALIPIX_CAPTURE") else {
            return Ok(None);
        };
        let mut steps = VecDeque::new();
        if let Ok(id) = std::env::var("CALIPIX_CAPTURE_RUN") {
            steps.push_back(Step::Run(id));
        }
        if let Ok(script) = std::env::var("CALIPIX_CAPTURE_STEPS") {
            for text in script.split(';').filter(|s| !s.trim().is_empty()) {
                steps.push_back(Step::parse(text.trim())?);
            }
        }
        Ok(Some(Capture {
            path: path.into(),
            steps,
            settled: 0,
            hold_until: None,
            mid_run: false,
            requested: false,
            pointer: VecDeque::new(),
            held: egui::Modifiers::NONE,
        }))
    }

    /// Advances the script by at most one step. Returns true once the
    /// picture has been saved and the window asked to close.
    pub fn drive(&mut self, ctx: &egui::Context, shell: &mut Shell) -> bool {
        ctx.request_repaint();
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(shot) = shot {
            if let Err(e) = save_screenshot(&shot, &self.path) {
                eprintln!("error: could not save capture: {e:#}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return true;
        }

        self.settled += 1;
        let holding = self.hold_until.is_some_and(|until| Instant::now() < until);
        let next_waits = matches!(self.steps.front(), Some(Step::Wait(_)));
        let busy = (shell.state.running().is_some() || shell.view().loading)
            && !self.mid_run
            && !next_waits;
        if self.settled < SETTLE || holding || busy || self.requested || !self.pointer.is_empty() {
            return false;
        }
        self.settled = 0;
        self.hold_until = None;
        match self.steps.pop_front() {
            Some(step) => {
                self.mid_run = matches!(step, Step::Wait(_));
                self.apply(step, shell);
            }
            None => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                self.requested = true;
            }
        }
        false
    }

    /// Adds this frame's share of the scripted pointer events to the
    /// window's input, as if a person had moved and clicked the mouse.
    pub fn inject(&mut self, raw_input: &mut egui::RawInput) {
        if let Some(events) = self.pointer.pop_front() {
            raw_input.events.extend(events);
        }
    }

    /// `step` with its places, given in the active image's coordinates,
    /// turned into places in the window.
    fn on_image(step: Step, shell: &Shell) -> Option<Step> {
        let place = shell.view().place?;
        let at = |p: egui::Pos2| place.screen([p.x as f64, p.y as f64]);
        Some(match step {
            Step::Drag(from, to) => Step::Drag(at(from), at(to)),
            Step::Click(p) => Step::Click(at(p)),
            Step::Double(p) => Step::Double(at(p)),
            Step::Hover(p) => Step::Hover(at(p)),
            Step::Press(p) => Step::Press(at(p)),
            Step::Release(p) => Step::Release(at(p)),
            Step::Wheel(p, notches) => Step::Wheel(at(p), notches),
            Step::Path(places) => Step::Path(places.into_iter().map(at).collect()),
            other => other,
        })
    }

    fn apply(&mut self, step: Step, shell: &mut Shell) {
        let held = self.held;
        let button_at = move |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: held,
        };
        match step {
            Step::OnImage(step) => match Capture::on_image(*step, shell) {
                Some(step) => self.apply(step, shell),
                None => eprintln!("warning: a capture step on the image found no image on show"),
            },
            Step::Tool(tool) => shell.invoke(&tool.command_id(), &Args::new()),
            Step::Key(shortcut) => {
                let Some(key) = egui::Key::from_name(&shortcut.key) else {
                    return eprintln!("warning: capture step key:{} names no key", shortcut.key);
                };
                let modifiers = egui::Modifiers {
                    alt: shortcut.alt,
                    ctrl: shortcut.ctrl,
                    shift: shortcut.shift,
                    mac_cmd: false,
                    command: shortcut.ctrl,
                };
                let event = |pressed| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                };
                self.pointer.push_back(vec![event(true)]);
                self.pointer.push_back(vec![event(false)]);
            }
            Step::Mods(modifiers) => {
                self.held = modifiers;
                self.pointer
                    .push_back(vec![egui::Event::ModifiersChanged(modifiers)]);
            }
            Step::Press(at) => {
                self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                self.pointer.push_back(vec![button_at(at, true)]);
            }
            Step::Release(at) => {
                self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                self.pointer.push_back(vec![button_at(at, false)]);
            }
            Step::Path(places) => {
                let (Some(&first), Some(&last)) = (places.first(), places.last()) else {
                    return;
                };
                self.pointer
                    .push_back(vec![egui::Event::PointerMoved(first)]);
                self.pointer.push_back(vec![button_at(first, true)]);
                for pair in places.windows(2) {
                    // A few steps along each leg, as a hand would move.
                    for step in 1..=4 {
                        let at = pair[0] + (pair[1] - pair[0]) * (step as f32 / 4.0);
                        self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                    }
                }
                self.pointer.push_back(vec![button_at(last, false)]);
            }
            Step::Wheel(at, notches) => {
                self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                for _ in 0..notches.abs() as usize {
                    self.pointer.push_back(vec![egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, notches.signum()),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    }]);
                }
            }
            Step::Regions(count) => {
                let Some(dataset) = shell.state.active() else {
                    return eprintln!("warning: capture step regions needs an open image");
                };
                let meta = dataset.image.meta();
                let entries = crate::demo::regions(count, meta.width(), meta.height());
                let path = std::env::temp_dir().join(format!(
                    "calipix-capture-regions-{}-{count}.zip",
                    std::process::id()
                ));
                let written = calipix_roi::write_zip(&entries)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
                match written {
                    Ok(()) => shell
                        .state
                        .start("regions.open", &Args::new().with("path", path)),
                    Err(e) => eprintln!("warning: capture step regions failed: {e}"),
                }
            }
            Step::Pick(first, last) => {
                let (first, last) = (first.saturating_sub(1), last.saturating_sub(1));
                shell.state.click_region(first, RowClick::default());
                if last != first {
                    let extend = RowClick {
                        extend: true,
                        toggle: false,
                    };
                    shell.state.click_region(last, extend);
                }
            }
            Step::Hover(at) => self.pointer.push_back(vec![egui::Event::PointerMoved(at)]),
            Step::Double(at) => {
                let button = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: held,
                };
                self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                for _ in 0..2 {
                    self.pointer.push_back(vec![button(true)]);
                    self.pointer.push_back(vec![button(false)]);
                }
            }
            Step::Click(at) => {
                let button = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: held,
                };
                self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                self.pointer.push_back(vec![button(true)]);
                self.pointer.push_back(vec![button(false)]);
            }
            Step::Open(path) => shell.state.open_files([path]),
            Step::Run(id) => shell.invoke(&id, &Args::new()),
            Step::Start(id, given) => {
                let Some(spec) = shell.state.spec(&id).cloned() else {
                    return eprintln!("warning: capture step start:{id} names no command");
                };
                let mut args = Args::new();
                for (name, text) in given {
                    let value = spec
                        .param(&name)
                        .ok_or_else(|| format!("no parameter `{name}`"))
                        .and_then(|param| calipix_command::Value::parse_text(&param.ty, &text));
                    match value {
                        Ok(value) => args.insert(name, value),
                        Err(e) => eprintln!("warning: capture step start:{id}: {e}"),
                    }
                }
                shell.state.start(&id, &args);
            }
            Step::Palette(query) => {
                shell.palette.open();
                shell.palette.query = query;
            }
            Step::Set(name, value) => {
                let field = shell.dialog.as_mut().and_then(|d| d.field_mut(&name));
                match field.map(|f| &mut f.draft) {
                    Some(Draft::Bool(on)) => *on = value == "true",
                    Some(Draft::Int(number)) => *number = value.parse().unwrap_or(*number),
                    Some(Draft::Float(number)) => *number = value.parse().unwrap_or(*number),
                    Some(Draft::Text(text)) => *text = value,
                    Some(Draft::Image(_)) | None => {
                        eprintln!("warning: capture step set:{name} had nothing to set");
                    }
                }
            }
            Step::More => {
                if let Some(dialog) = &mut shell.dialog {
                    dialog.show_advanced = true;
                }
            }
            Step::Accept => shell.accept_dialog(),
            Step::Select(n) => {
                let id = shell
                    .state
                    .session()
                    .workspace
                    .datasets()
                    .nth(n.saturating_sub(1))
                    .map(|dataset| dataset.id);
                if let Some(id) = id {
                    shell.state.set_active(id);
                }
            }
            Step::Position(position) => {
                if let Some(id) = shell.state.active().map(|dataset| dataset.id) {
                    shell.state.set_position(id, &position);
                }
            }
            Step::Visible(channel, visible) => {
                if let Some(id) = shell.state.active().map(|dataset| dataset.id) {
                    shell
                        .state
                        .set_channel_visible(id, channel.saturating_sub(1), visible);
                }
            }
            Step::Drag(from, to) => {
                let button = button_at;
                self.pointer
                    .push_back(vec![egui::Event::PointerMoved(from)]);
                self.pointer.push_back(vec![button(from, true)]);
                // In steps, as a hand would move, so that it is a drag
                // and not a click.
                for step in 1..=6 {
                    let at = from + (to - from) * (step as f32 / 6.0);
                    self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                }
                self.pointer.push_back(vec![button(to, false)]);
            }
            Step::Panel(name) => match Panel::ALL.into_iter().find(|p| p.key() == name) {
                Some(panel) => {
                    shell.layout.set_open(panel, true);
                    shell.layout.focus(Pane::Panel(panel));
                }
                None => eprintln!("warning: capture step panel:{name} names no panel"),
            },
            Step::Table(name) => shell.state.show_table(&name),
            Step::Plot(name) => shell.state.show_plot(&name),
            Step::Rows(first, last) => {
                let (first, last) = (first.saturating_sub(1), last.saturating_sub(1));
                shell.state.click_row(first, RowClick::default());
                let extend = RowClick {
                    extend: true,
                    toggle: false,
                };
                shell.state.click_row(last, extend);
            }
            Step::Log => {
                let state = &shell.state;
                eprintln!("capture: status {:?}", state.status);
                for (index, row) in state.history_rows().iter().enumerate() {
                    let mark = if row.undone { " (undone)" } else { "" };
                    eprintln!(
                        "capture: history {} {}{mark}: {}",
                        index + 1,
                        row.title,
                        row.detail
                    );
                }
                let view = shell.view();
                eprintln!(
                    "capture: view zoom {:?}, drawn on the {}, probe {:?}",
                    view.zoom,
                    if view.on_cpu { "CPU" } else { "GPU" },
                    view.probe.as_ref().map(super::status::probe_text),
                );
                for entry in state.log() {
                    eprintln!("capture: log {:?} {}", entry.level, entry.text);
                }
                let regions = view.regions;
                eprintln!(
                    "capture: tool {}, selection {:?}",
                    state.tool().title(),
                    state
                        .active()
                        .and_then(|d| d.roi.as_ref())
                        .map(|r| r.to_string()),
                );
                eprintln!(
                    "capture: regions {}, {} outlined, {} as dots, {} labels, laid out in {} µs",
                    regions.regions, regions.outlined, regions.dots, regions.labels, regions.micros,
                );
                eprintln!(
                    "capture: the last frame took {:.1} ms of processor time",
                    shell.frame_seconds.unwrap_or(0.0) * 1000.0,
                );
                if let Some(place) = view.place {
                    eprintln!(
                        "capture: image at {:?}, its corner at {:?}",
                        place.rect,
                        place.screen([0.0, 0.0])
                    );
                }
                eprintln!(
                    "capture: tables {:?}, showing {:?}; plots {:?}, showing {:?}",
                    state.table_names(),
                    state.shown_table(),
                    state.plot_names(),
                    state.shown_plot(),
                );
            }
            Step::Wait(time) => self.hold_until = Some(Instant::now() + time),
        }
    }
}

fn save_screenshot(shot: &egui::ColorImage, path: &std::path::Path) -> Result<()> {
    let [width, height] = shot.size;
    let mut meta = ImageMeta::xyczt(DType::U8, width, height, 3, 1, 1)?;
    meta.color = ColorModel::Rgb;
    let planes = (0..3)
        .map(|c| {
            Buffer::from_vec(
                shot.pixels
                    .iter()
                    .map(|p| p.to_array()[c])
                    .collect::<Vec<u8>>(),
            )
        })
        .collect();
    calipix_io::save(&Image::from_planes(meta, planes)?, path)?;
    Ok(())
}
