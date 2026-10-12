//! Unattended capture of the window, for checking the interface without a
//! person at the screen.
//!
//! Setting `CALIPIX_CAPTURE` to a PNG path makes the application carry out
//! the steps in `CALIPIX_CAPTURE_STEPS`, save a picture of its window there
//! and exit. Steps are separated by `;`:
//!
//! - `open:<path>` opens an image file
//! - `run:<command id>` chooses a command as from a menu
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
use crate::state::{Draft, Panel, RowClick};

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Open(PathBuf),
    Run(String),
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
        if self.settled < SETTLE || holding || busy || self.requested {
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

    fn apply(&mut self, step: Step, shell: &mut Shell) {
        match step {
            Step::Hover(at) => self.pointer.push_back(vec![egui::Event::PointerMoved(at)]),
            Step::Double(at) => {
                let button = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
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
                    modifiers: egui::Modifiers::NONE,
                };
                self.pointer.push_back(vec![egui::Event::PointerMoved(at)]);
                self.pointer.push_back(vec![button(true)]);
                self.pointer.push_back(vec![button(false)]);
            }
            Step::Open(path) => shell.state.open_files([path]),
            Step::Run(id) => shell.invoke(&id, &Args::new()),
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
                let button = |pos, pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
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
