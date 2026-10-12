//! The contents of a parameter dialog, generated from a command's spec.
//! This holds what the person has entered so far, says whether it is valid
//! and turns it into arguments; the interface only draws it.

use std::collections::BTreeMap;
use std::path::Path;

use calipix_command::{
    Args, CommandSpec, DatasetId, Error, ParamSpec, ParamType, PathMode, PositionAxis, Value,
    Workspace,
};
use serde_json::Value as Json;

/// A value being edited, in the form its control works on.
#[derive(Clone, Debug, PartialEq)]
pub enum Draft {
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Strings, choices, paths, lists as comma-separated text, and
    /// numbers that may be left out (empty text means "not given").
    Text(String),
    Image(Option<DatasetId>),
}

/// One row of a dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub param: ParamSpec,
    pub draft: Draft,
}

impl Field {
    /// The entered value, `None` if an optional parameter was left empty,
    /// or the reason the entry cannot be used.
    pub fn value(&self, workspace: &Workspace) -> Result<Option<Value>, String> {
        let param = &self.param;
        let value = match (&self.draft, &param.ty) {
            (Draft::Bool(b), _) => Value::Bool(*b),
            (Draft::Int(i), _) => Value::Int(*i),
            (Draft::Float(f), _) => Value::Float(*f),
            (Draft::Image(None), _) if !param.required => return Ok(None),
            (Draft::Image(None), _) if workspace.is_empty() => {
                return Err("Open an image first".to_string());
            }
            (Draft::Image(None), _) => return Err("Choose an image".to_string()),
            (Draft::Image(Some(id)), _) => {
                if workspace.get(*id).is_err() {
                    return Err("This image is no longer open".to_string());
                }
                Value::Image(*id)
            }
            (Draft::Text(text), ParamType::Path(mode)) => {
                if text.trim().is_empty() {
                    if !param.required {
                        return Ok(None);
                    }
                    return Err(match mode {
                        PathMode::Directory => "Choose a folder",
                        PathMode::Open | PathMode::Save => "Choose a file",
                    }
                    .to_string());
                }
                let path = Path::new(text.trim());
                match mode {
                    PathMode::Open if !path.is_file() => {
                        return Err("There is no file at this path".to_string());
                    }
                    PathMode::Directory if !path.is_dir() => {
                        return Err("There is no folder at this path".to_string());
                    }
                    _ => {}
                }
                Value::Path(path.to_path_buf())
            }
            (Draft::Text(text), ParamType::List(_)) => {
                if text.trim().is_empty() && !param.required {
                    return Ok(None);
                }
                let items: Vec<&str> = text.split(',').map(str::trim).collect();
                if let ParamType::List(inner) = &param.ty
                    && let Some(problem) = items.iter().find_map(|item| not_a_number(inner, item))
                {
                    return Err(problem);
                }
                Value::parse_text(&param.ty, &items.join(",")).map_err(|e| sentence(&e))?
            }
            (Draft::Text(text), ParamType::Str) if text.is_empty() && !param.required => {
                return Ok(None);
            }
            (Draft::Text(text), ParamType::Int | ParamType::Float) => {
                let text = text.trim();
                if text.is_empty() {
                    if !param.required {
                        return Ok(None);
                    }
                    return Err("Enter a number".to_string());
                }
                match not_a_number(&param.ty, text) {
                    Some(problem) => return Err(problem),
                    None => Value::parse_text(&param.ty, text).map_err(|e| sentence(&e))?,
                }
            }
            (Draft::Text(text), _) => Value::Str(text.clone()),
        };
        if let Some(problem) = out_of_range(param, &value) {
            return Err(problem);
        }
        match param.check(value) {
            Ok(value) => Ok(Some(value)),
            Err(Error::InvalidArg { reason, .. }) => Err(sentence(&reason)),
            Err(other) => Err(sentence(&other.to_string())),
        }
    }

    /// Why the entry cannot be used, if it cannot.
    pub fn problem(&self, workspace: &Workspace) -> Option<String> {
        self.value(workspace).err()
    }
}

/// Why `item` cannot be an element of a list of numbers, if it cannot.
fn not_a_number(inner: &ParamType, item: &str) -> Option<String> {
    let kind = match inner {
        ParamType::Int => "a whole number",
        ParamType::Float => "a number",
        _ => return None,
    };
    Value::parse_text(inner, item).is_err().then(|| match item {
        "" => format!("Each value must be {kind}; one is missing"),
        _ => format!("“{item}” is not {kind}"),
    })
}

/// Says which bound a number breaks, in the words of someone correcting it.
fn out_of_range(param: &ParamSpec, value: &Value) -> Option<String> {
    let number = match value {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        _ => return None,
    };
    let below = param.min.is_some_and(|min| number < min);
    let above = param.max.is_some_and(|max| number > max);
    match (param.min, param.max) {
        _ if !below && !above => None,
        (Some(min), Some(max)) => Some(format!("Must be between {min} and {max}")),
        (Some(min), None) => Some(format!("Must be at least {min}")),
        (None, Some(max)) => Some(format!("Must be at most {max}")),
        (None, None) => None,
    }
}

/// Capitalises a message from the command system for display on its own.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// True for a number that may be left out. It is edited as text, so that
/// the field can be empty: zero is a value, not the absence of one.
fn optional_number(param: &ParamSpec) -> bool {
    !param.required && matches!(param.ty, ParamType::Int | ParamType::Float)
}

/// A number as a field shows it: `2`, not `2.0`, and `2.5`.
pub fn number_text(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        // The shortest text that reads back as the same number.
        format!("{value}")
    }
}

/// The draft a parameter starts from when nothing better is known.
fn blank(param: &ParamSpec) -> Draft {
    // Zero, moved into the allowed range.
    let within = |zero: f64| {
        zero.max(param.min.unwrap_or(zero))
            .min(param.max.unwrap_or(zero))
    };
    match &param.ty {
        _ if optional_number(param) => Draft::Text(String::new()),
        ParamType::Bool => Draft::Bool(false),
        ParamType::Int => Draft::Int(within(0.0) as i64),
        ParamType::Float => Draft::Float(within(0.0)),
        ParamType::Choice(choices) => Draft::Text(choices.first().cloned().unwrap_or_default()),
        ParamType::Image => Draft::Image(None),
        _ => Draft::Text(String::new()),
    }
}

fn draft_of(param: &ParamSpec, value: &Value) -> Option<Draft> {
    Some(match (param.check(value.clone()).ok()?, &param.ty) {
        (Value::Int(i), _) if optional_number(param) => Draft::Text(i.to_string()),
        (Value::Float(f), _) if optional_number(param) => Draft::Text(number_text(f)),
        (Value::Bool(b), _) => Draft::Bool(b),
        (Value::Int(i), _) => Draft::Int(i),
        (Value::Float(f), _) => Draft::Float(f),
        (Value::Image(id), _) => Draft::Image(Some(id)),
        (list @ Value::List(_), _) => Draft::Text(list.to_text().replace(',', ", ")),
        (other, _) => Draft::Text(other.to_text()),
    })
}

/// Last used values of one command, by parameter name.
pub type Remembered = BTreeMap<String, Json>;

#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub spec: CommandSpec,
    pub fields: Vec<Field>,
    /// Arguments fixed by whoever opened the dialog.
    preset: Args,
    /// Whether the "More options" section is unfolded.
    pub show_advanced: bool,
}

impl Dialog {
    /// Starts a dialog for `spec`. Each field begins with, in order of
    /// preference: the preset argument, the value remembered from last
    /// time, the active image (image parameters), the spec's default.
    pub fn new(
        spec: &CommandSpec,
        workspace: &Workspace,
        preset: &Args,
        remembered: Option<&Remembered>,
    ) -> Dialog {
        let mut dialog = Dialog {
            spec: spec.clone(),
            fields: Vec::new(),
            preset: preset.clone(),
            show_advanced: false,
        };
        dialog.reset(workspace);
        for field in &mut dialog.fields {
            let param = &field.param;
            let recalled = remembered
                .filter(|_| remembers(param))
                .and_then(|values| values.get(&param.name))
                .and_then(|json| Value::from_json(&param.ty, json).ok());
            if let Some(draft) = recalled.and_then(|value| draft_of(param, &value)) {
                field.draft = draft;
            }
        }
        dialog.apply_preset(workspace);
        dialog
    }

    fn apply_preset(&mut self, workspace: &Workspace) {
        for field in &mut self.fields {
            let given = self.preset.get(&field.param.name);
            if let Some(draft) = given.and_then(|value| draft_of(&field.param, value)) {
                field.draft = draft;
            }
            // A remembered or preset image may have been closed since.
            if let Draft::Image(Some(id)) = field.draft
                && workspace.get(id).is_err()
            {
                field.draft = Draft::Image(workspace.active());
            }
        }
    }

    /// Restores the command's defaults, forgetting remembered values.
    pub fn reset(&mut self, workspace: &Workspace) {
        // What the view has already answered is not something to ask
        // about; its value travels in the preset. That is the plane on
        // show, "whole stack?" for an image with a single plane, and the
        // image itself when the command takes only one.
        let preset = self.preset.clone();
        let images = self.spec.inputs.iter().filter(|p| p.ty == ParamType::Image);
        let only_image = images.count() == 1;
        let answered = |param: &ParamSpec| {
            (param.position.is_some() || (param.ty == ParamType::Image && only_image))
                && preset.get(&param.name).is_some()
        };
        // Image parameters start on different images where there are
        // several to choose from: the active one, then the others, most
        // recently opened first.
        let mut candidates: Vec<DatasetId> = workspace.active().into_iter().collect();
        let others: Vec<DatasetId> = workspace.datasets().map(|d| d.id).collect();
        candidates.extend(
            others
                .into_iter()
                .rev()
                .filter(|id| Some(*id) != workspace.active()),
        );
        let mut next_image = 0;
        self.fields = self
            .spec
            .inputs
            .iter()
            .filter(|param| !answered(param))
            .map(|param| {
                let draft = match (&param.ty, &param.default) {
                    (ParamType::Image, _) if param.required => {
                        let image = candidates.get(next_image).or(candidates.first());
                        next_image += 1;
                        Draft::Image(image.copied())
                    }
                    // An image that may be left out is the active one if it
                    // is, as the executor would fill it in.
                    (ParamType::Image, _) => Draft::Image(workspace.active()),
                    (_, Some(default)) => draft_of(param, default).unwrap_or_else(|| blank(param)),
                    (_, None) => blank(param),
                };
                Field {
                    param: param.clone(),
                    draft,
                }
            })
            .collect();
        // "Whole stack?" is asked after what the command itself needs to
        // know, though commands declare it with the image, first.
        let is_stack = |field: &Field| {
            field.param.position == Some(PositionAxis::Stack) && !field.param.advanced
        };
        if self.fields.first().is_some_and(is_stack) {
            let last_basic = self.fields.iter().rposition(|f| !f.param.advanced);
            let stack = self.fields.remove(0);
            self.fields.insert(last_basic.unwrap_or(0), stack);
        }
        // Two images to choose are told apart by number.
        if self.fields.iter().any(|f| f.param.label == "Image 2") {
            for field in &mut self.fields {
                if field.param.ty == ParamType::Image && field.param.label == "Image" {
                    field.param.label = "Image 1".to_string();
                }
            }
        }
        self.apply_preset(workspace);
    }

    pub fn field_mut(&mut self, name: &str) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.param.name == name)
    }

    /// True if a parameter's text runs over several lines, so that Enter
    /// in its field starts a new line rather than accepting the dialog.
    pub fn has_multiline(&self) -> bool {
        self.fields.iter().any(|f| f.param.multiline)
    }

    pub fn has_advanced(&self) -> bool {
        self.fields.iter().any(|f| f.param.advanced)
    }

    /// True if every field holds a usable value, so OK may be pressed.
    pub fn is_valid(&self, workspace: &Workspace) -> bool {
        self.fields.iter().all(|f| f.value(workspace).is_ok())
    }

    /// The arguments to run the command with, or `None` while any field is
    /// invalid.
    pub fn args(&self, workspace: &Workspace) -> Option<Args> {
        // Start from what was decided before the dialog opened; fields
        // overwrite it.
        let mut args = Args::new();
        for (name, value) in self.preset.iter() {
            if self.spec.param(name).is_some() {
                args.insert(name, value.clone());
            }
        }
        for field in &self.fields {
            if let Some(value) = field.value(workspace).ok()? {
                args.insert(field.param.name.clone(), value);
            }
        }
        Some(args)
    }

    /// The values to remember for next time. Images are left out: their
    /// ids mean nothing in another session.
    pub fn remember(&self, workspace: &Workspace) -> Remembered {
        self.fields
            .iter()
            .filter(|field| remembers(&field.param))
            .filter_map(|field| {
                let value = field.value(workspace).ok()??;
                Some((field.param.name.clone(), value.to_json()))
            })
            .collect()
    }
}

fn remembers(param: &ParamSpec) -> bool {
    param.persist && !contains_image(&param.ty)
}

fn contains_image(ty: &ParamType) -> bool {
    match ty {
        ParamType::Image => true,
        ParamType::List(inner) => contains_image(inner),
        _ => false,
    }
}
