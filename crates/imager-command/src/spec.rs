use serde_json::{Map, Value as Json, json};

use crate::{Error, ParamType, PathMode, Result, Value};

/// Description of one command input.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    /// Stable machine name, `snake_case`.
    pub name: String,
    /// Label shown next to the control in dialogs.
    pub label: String,
    pub doc: String,
    pub ty: ParamType,
    pub default: Option<Value>,
    /// If false, the command accepts the parameter being absent.
    pub required: bool,
    /// Inclusive bounds for numeric parameters.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Suggested increment for sliders and spinners.
    pub step: Option<f64>,
    pub unit: Option<String>,
    /// Shown only in the expanded part of a dialog.
    pub advanced: bool,
    /// Whether the last used value is remembered between runs.
    pub persist: bool,
    /// Keyword for this parameter in ImageJ macro option strings, if it
    /// differs from `name`.
    pub ijm_key: Option<String>,
}

impl ParamSpec {
    pub fn new(name: impl Into<String>, ty: ParamType) -> ParamSpec {
        let name = name.into();
        ParamSpec {
            label: humanize(&name),
            name,
            doc: String::new(),
            ty,
            default: None,
            required: true,
            min: None,
            max: None,
            step: None,
            unit: None,
            advanced: false,
            persist: true,
            ijm_key: None,
        }
    }

    /// Replaces the parameter type, e.g. to narrow a string to a choice.
    /// Call before [`default_value`](Self::default_value).
    pub fn with_type(mut self, ty: ParamType) -> ParamSpec {
        self.ty = ty;
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> ParamSpec {
        self.label = label.into();
        self
    }

    pub fn doc(mut self, doc: impl Into<String>) -> ParamSpec {
        self.doc = doc.into();
        self
    }

    /// Sets the default. A value of the wrong type is kept as given and
    /// reported by [`CommandSpec::validate`].
    pub fn default_value(mut self, value: impl Into<Value>) -> ParamSpec {
        let value = value.into();
        self.default = Some(value.clone().coerce(&self.ty).unwrap_or(value));
        self
    }

    pub fn required(mut self, required: bool) -> ParamSpec {
        self.required = required;
        self
    }

    pub fn min(mut self, min: f64) -> ParamSpec {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> ParamSpec {
        self.max = Some(max);
        self
    }

    pub fn step(mut self, step: f64) -> ParamSpec {
        self.step = Some(step);
        self
    }

    pub fn unit(mut self, unit: impl Into<String>) -> ParamSpec {
        self.unit = Some(unit.into());
        self
    }

    pub fn advanced(mut self, advanced: bool) -> ParamSpec {
        self.advanced = advanced;
        self
    }

    pub fn persist(mut self, persist: bool) -> ParamSpec {
        self.persist = persist;
        self
    }

    pub fn ijm_key(mut self, key: impl Into<String>) -> ParamSpec {
        self.ijm_key = Some(key.into());
        self
    }

    /// Converts `value` to this parameter's type and checks its bounds.
    pub fn check(&self, value: Value) -> Result<Value> {
        let value = value
            .coerce(&self.ty)
            .map_err(|reason| Error::invalid_arg(&self.name, reason))?;
        let number = match value {
            Value::Int(i) => Some(i as f64),
            Value::Float(f) => Some(f),
            _ => None,
        };
        if let Some(n) = number {
            if n.is_nan() {
                return Err(Error::invalid_arg(&self.name, "not a number"));
            }
            if self.min.is_some_and(|min| n < min) || self.max.is_some_and(|max| n > max) {
                let bound = |b: Option<f64>| b.map_or("..".to_string(), |b| b.to_string());
                return Err(Error::invalid_arg(
                    &self.name,
                    format!(
                        "{n} is outside the allowed range {} to {}",
                        bound(self.min),
                        bound(self.max)
                    ),
                ));
            }
        }
        Ok(value)
    }

    fn json_schema(&self) -> Json {
        let mut schema = self.ty.json_schema();
        let obj = schema.as_object_mut().expect("type schemas are objects");
        obj.insert("title".to_string(), json!(self.label));
        let mut description = self.doc.clone();
        if let Some(unit) = &self.unit {
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(&format!("Unit: {unit}."));
        }
        if !description.is_empty() {
            obj.insert("description".to_string(), json!(description));
        }
        if let Some(default) = &self.default {
            obj.insert("default".to_string(), default.to_json());
        }
        if let Some(min) = self.min {
            obj.insert("minimum".to_string(), json!(min));
        }
        if let Some(max) = self.max {
            obj.insert("maximum".to_string(), json!(max));
        }
        schema
    }

    fn to_json(&self) -> Json {
        json!({
            "name": self.name,
            "label": self.label,
            "doc": self.doc,
            "type": self.ty.name(),
            "choices": match &self.ty {
                ParamType::Choice(choices) => json!(choices),
                _ => Json::Null,
            },
            "default": self.default.as_ref().map(Value::to_json),
            "required": self.required,
            "min": self.min,
            "max": self.max,
            "step": self.step,
            "unit": self.unit,
            "advanced": self.advanced,
        })
    }
}

/// `sigma_x` becomes `Sigma x`.
fn humanize(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Description of one command output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputSpec {
    pub name: String,
    pub label: String,
    pub ty: ParamType,
}

impl OutputSpec {
    pub fn new(name: impl Into<String>, ty: ParamType) -> OutputSpec {
        let name = name.into();
        OutputSpec {
            label: humanize(&name),
            name,
            ty,
        }
    }
}

/// What a command does to open images.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageEffect {
    /// Only reads images, if it touches them at all.
    #[default]
    None,
    /// Replaces the contents of an input image.
    Modifies,
    /// Adds a new image to the workspace.
    Creates,
}

/// Side effects a command may have. Frontends use these to decide what
/// needs approval, and the executor refuses commands whose effects the
/// current policy does not allow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub image: ImageEffect,
    pub reads_fs: bool,
    pub writes_fs: bool,
    pub network: bool,
}

/// Behavioural properties of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags {
    /// Supports a live preview while its dialog is open.
    pub previewable: bool,
    /// Can run without a user interface.
    pub headless_ok: bool,
    /// Checks for cancellation while running.
    pub cancellable: bool,
    /// Appears in history and in recorded macros.
    pub recordable: bool,
}

impl Default for Flags {
    fn default() -> Flags {
        Flags {
            previewable: false,
            headless_ok: true,
            cancellable: false,
            recordable: true,
        }
    }
}

/// Where a command sits in the menus. `path` lists the enclosing menus,
/// outermost first; the item itself is labelled with the command's title.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuPath {
    pub path: Vec<String>,
    /// Orders items within a menu, lowest first.
    pub weight: f64,
}

/// The single definition of a command from which every frontend is derived.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandSpec {
    /// Stable identifier of the form `group.name`, e.g. `filter.gaussian_blur`.
    pub id: String,
    /// Name shown in menus and the command palette.
    pub title: String,
    /// One sentence saying what the command does.
    pub summary: String,
    /// Longer help text.
    pub doc: String,
    pub menu: Option<MenuPath>,
    pub inputs: Vec<ParamSpec>,
    pub outputs: Vec<OutputSpec>,
    pub effects: Effects,
    pub flags: Flags,
    /// The command's name in ImageJ, as used by `run("...")` in macros and
    /// as a search alias.
    pub ijm_name: Option<String>,
    /// Keyboard shortcut such as `Ctrl+Shift+S`. `Ctrl` means Cmd on macOS.
    pub shortcut: Option<String>,
}

impl CommandSpec {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> CommandSpec {
        CommandSpec {
            id: id.into(),
            title: title.into(),
            summary: String::new(),
            doc: String::new(),
            menu: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            effects: Effects::default(),
            flags: Flags::default(),
            ijm_name: None,
            shortcut: None,
        }
    }

    pub fn summary(mut self, summary: impl Into<String>) -> CommandSpec {
        self.summary = summary.into();
        self
    }

    pub fn doc(mut self, doc: impl Into<String>) -> CommandSpec {
        self.doc = doc.into();
        self
    }

    /// Places the command in a menu given as `"Image > Adjust"`.
    pub fn menu(mut self, path: &str, weight: f64) -> CommandSpec {
        self.menu = Some(MenuPath {
            path: path.split('>').map(|s| s.trim().to_string()).collect(),
            weight,
        });
        self
    }

    pub fn input(mut self, param: ParamSpec) -> CommandSpec {
        self.inputs.push(param);
        self
    }

    pub fn output(mut self, name: impl Into<String>, ty: ParamType) -> CommandSpec {
        self.outputs.push(OutputSpec::new(name, ty));
        self
    }

    pub fn effects(mut self, effects: Effects) -> CommandSpec {
        self.effects = effects;
        self
    }

    pub fn flags(mut self, flags: Flags) -> CommandSpec {
        self.flags = flags;
        self
    }

    pub fn ijm_name(mut self, name: impl Into<String>) -> CommandSpec {
        self.ijm_name = Some(name.into());
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<String>) -> CommandSpec {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn param(&self, name: &str) -> Option<&ParamSpec> {
        self.inputs.iter().find(|p| p.name == name)
    }

    /// Checks the definition for internal consistency.
    pub fn validate(&self) -> Result<()> {
        let fail = |reason: String| {
            Err(Error::InvalidSpec {
                id: self.id.clone(),
                reason,
            })
        };
        let parts: Vec<&str> = self.id.split('.').collect();
        if parts.len() < 2 || !parts.iter().all(|p| is_identifier(p)) {
            return fail("id must be dot-separated snake_case words, e.g. `filter.blur`".into());
        }
        if self.title.trim().is_empty() {
            return fail("title is empty".into());
        }
        if self.title.ends_with("...") || self.title.ends_with('…') {
            return fail("title ends with an ellipsis; the interface adds one when needed".into());
        }
        if let Some(shortcut) = &self.shortcut
            && !is_shortcut(shortcut)
        {
            return fail(format!(
                "`{shortcut}` is not a shortcut like `Ctrl+Shift+S`"
            ));
        }
        if let Some(menu) = &self.menu
            && menu.path.iter().any(|p| p.is_empty())
        {
            return fail("menu path has an empty component".into());
        }
        for (i, param) in self.inputs.iter().enumerate() {
            let name = &param.name;
            if !is_identifier(name) {
                return fail(format!("parameter name `{name}` is not snake_case"));
            }
            if self.inputs[..i].iter().any(|p| &p.name == name) {
                return fail(format!("duplicate parameter `{name}`"));
            }
            if let ParamType::Choice(choices) = &param.ty
                && choices.is_empty()
            {
                return fail(format!("parameter `{name}` has no choices"));
            }
            if let (Some(min), Some(max)) = (param.min, param.max)
                && min > max
            {
                return fail(format!("parameter `{name}` has min > max"));
            }
            let numeric = matches!(param.ty, ParamType::Int | ParamType::Float);
            if !numeric && (param.min.is_some() || param.max.is_some() || param.step.is_some()) {
                return fail(format!("parameter `{name}` has bounds but is not numeric"));
            }
            if let Some(default) = &param.default
                && let Err(e) = param.check(default.clone())
            {
                return fail(format!("default of `{name}` is invalid: {e}"));
            }
            match param.ty {
                ParamType::Path(PathMode::Open) if !self.effects.reads_fs => {
                    return fail(format!(
                        "`{name}` opens a path but reads_fs is not declared"
                    ));
                }
                ParamType::Path(PathMode::Save) if !self.effects.writes_fs => {
                    return fail(format!(
                        "`{name}` saves a path but writes_fs is not declared"
                    ));
                }
                _ => {}
            }
        }
        for (i, output) in self.outputs.iter().enumerate() {
            if !is_identifier(&output.name) {
                return fail(format!("output name `{}` is not snake_case", output.name));
            }
            if self.outputs[..i].iter().any(|o| o.name == output.name) {
                return fail(format!("duplicate output `{}`", output.name));
            }
        }
        Ok(())
    }

    /// JSON Schema for the command's arguments, suitable as a tool input
    /// schema.
    pub fn input_schema(&self) -> Json {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for param in &self.inputs {
            properties.insert(param.name.clone(), param.json_schema());
            if param.required && param.default.is_none() {
                required.push(json!(param.name));
            }
        }
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    /// Complete machine-readable description of the command.
    pub fn to_json(&self) -> Json {
        json!({
            "id": self.id,
            "title": self.title,
            "summary": self.summary,
            "doc": self.doc,
            "menu": self.menu.as_ref().map(|m| json!({ "path": m.path, "weight": m.weight })),
            "inputs": self.inputs.iter().map(ParamSpec::to_json).collect::<Vec<_>>(),
            "outputs": self.outputs.iter().map(|o| json!({
                "name": o.name,
                "label": o.label,
                "type": o.ty.name(),
            })).collect::<Vec<_>>(),
            "effects": {
                "image": match self.effects.image {
                    ImageEffect::None => "none",
                    ImageEffect::Modifies => "modifies",
                    ImageEffect::Creates => "creates",
                },
                "reads_fs": self.effects.reads_fs,
                "writes_fs": self.effects.writes_fs,
                "network": self.effects.network,
            },
            "flags": {
                "previewable": self.flags.previewable,
                "headless_ok": self.flags.headless_ok,
                "cancellable": self.flags.cancellable,
                "recordable": self.flags.recordable,
            },
            "ijm_name": self.ijm_name,
            "shortcut": self.shortcut,
            "input_schema": self.input_schema(),
        })
    }
}

/// Zero or more of Ctrl, Shift and Alt, then one key, joined by `+`.
fn is_shortcut(s: &str) -> bool {
    // The `+` key itself is written last, as in `Ctrl++`.
    let (modifiers, key) = match s.strip_suffix('+') {
        Some(rest) if rest.is_empty() || rest.ends_with('+') => {
            (rest.strip_suffix('+').unwrap_or(rest), "+")
        }
        Some(_) => return false,
        None => match s.rsplit_once('+') {
            Some((modifiers, key)) => (modifiers, key),
            None => ("", s),
        },
    };
    let mut seen = Vec::new();
    if !modifiers.is_empty() {
        for m in modifiers.split('+') {
            if !["Ctrl", "Shift", "Alt"].contains(&m) || seen.contains(&m) {
                return false;
            }
            seen.push(m);
        }
    }
    let named = [
        "Enter",
        "Escape",
        "Space",
        "Tab",
        "Delete",
        "Backspace",
        "Left",
        "Right",
        "Up",
        "Down",
        "Home",
        "End",
        "PageUp",
        "PageDown",
    ];
    let function_key = key
        .strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=12).contains(&n));
    let mut chars = key.chars();
    let single = matches!(
        (chars.next(), chars.next()),
        (Some(c), None) if c.is_ascii_uppercase() || c.is_ascii_digit() || c.is_ascii_punctuation()
    );
    single || function_key || named.contains(&key)
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blur() -> CommandSpec {
        CommandSpec::new("filter.gaussian_blur", "Gaussian Blur")
            .summary("Smooths an image.")
            .menu("Process > Filters", 10.0)
            .input(ParamSpec::new("input", ParamType::Image))
            .input(
                ParamSpec::new("sigma", ParamType::Float)
                    .default_value(2)
                    .min(0.0)
                    .unit("pixels"),
            )
    }

    #[test]
    fn valid_spec_passes() {
        let spec = blur();
        spec.validate().unwrap();
        assert_eq!(spec.menu.as_ref().unwrap().path, ["Process", "Filters"]);
        assert_eq!(spec.param("sigma").unwrap().label, "Sigma");
        // The integer default was coerced to the parameter's type.
        assert_eq!(
            spec.param("sigma").unwrap().default,
            Some(Value::Float(2.0))
        );
    }

    #[test]
    fn invalid_specs_are_rejected() {
        assert!(CommandSpec::new("blur", "Blur").validate().is_err());
        assert!(CommandSpec::new("Filter.Blur", "Blur").validate().is_err());
        assert!(CommandSpec::new("filter.blur", " ").validate().is_err());
        assert!(
            CommandSpec::new("filter.blur", "Blur...")
                .validate()
                .is_err()
        );
        for good in [
            "Ctrl+O",
            "Ctrl+Shift+S",
            "F5",
            "+",
            "Ctrl++",
            "0",
            "Alt+Left",
            "Delete",
        ] {
            let spec = CommandSpec::new("filter.blur", "Blur").shortcut(good);
            assert!(spec.validate().is_ok(), "{good}");
        }
        for bad in [
            "ctrl+o",
            "Ctrl+",
            "Ctrl+Ctrl+O",
            "Cmd+O",
            "Ctrl+Open",
            "",
            "F13",
            "o",
            "+Ctrl+O",
        ] {
            let spec = CommandSpec::new("filter.blur", "Blur").shortcut(bad);
            assert!(spec.validate().is_err(), "{bad}");
        }
        let dup = blur().input(ParamSpec::new("sigma", ParamType::Float));
        assert!(dup.validate().is_err());
        let bad_default = blur().input(ParamSpec::new("n", ParamType::Int).default_value("x"));
        assert!(bad_default.validate().is_err());
        let out_of_range = blur().input(
            ParamSpec::new("n", ParamType::Int)
                .min(1.0)
                .default_value(0),
        );
        assert!(out_of_range.validate().is_err());
        let undeclared_fs = blur().input(ParamSpec::new("path", ParamType::Path(PathMode::Save)));
        assert!(undeclared_fs.validate().is_err());
    }

    #[test]
    fn check_enforces_bounds() {
        let sigma = blur().param("sigma").unwrap().clone();
        assert_eq!(sigma.check(Value::Int(3)).unwrap(), Value::Float(3.0));
        assert!(sigma.check(Value::Float(-1.0)).is_err());
        assert!(sigma.check(Value::Float(f64::NAN)).is_err());
        assert!(sigma.check(Value::from("wide")).is_err());
    }

    #[test]
    fn input_schema_lists_required_parameters() {
        let schema = blur().input_schema();
        assert_eq!(schema["required"], json!(["input"]));
        assert_eq!(schema["properties"]["sigma"]["type"], "number");
        assert_eq!(schema["properties"]["sigma"]["default"], 2.0);
        assert_eq!(schema["properties"]["sigma"]["minimum"], 0.0);
        assert_eq!(schema["additionalProperties"], false);
    }
}
