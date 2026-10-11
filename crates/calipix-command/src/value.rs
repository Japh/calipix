use std::fmt;
use std::path::PathBuf;

use serde_json::{Value as Json, json};

/// Identifies an open image within a [`Workspace`](crate::Workspace).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DatasetId(pub u64);

impl fmt::Display for DatasetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a path parameter is used for; decides which file dialog to show and
/// which filesystem effect the command needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMode {
    Open,
    Save,
    Directory,
}

/// The type of a command parameter or output.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ParamType {
    Bool,
    Int,
    Float,
    Str,
    /// One of a fixed set of strings.
    Choice(Vec<String>),
    Path(PathMode),
    /// An open image, referred to by its [`DatasetId`].
    Image,
    List(Box<ParamType>),
}

/// A command argument or output value.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Path(PathBuf),
    Image(DatasetId),
    List(Vec<Value>),
}

impl ParamType {
    /// Short name used in help text and the JSON form of a spec.
    pub fn name(&self) -> String {
        match self {
            ParamType::Bool => "bool".to_string(),
            ParamType::Int => "int".to_string(),
            ParamType::Float => "float".to_string(),
            ParamType::Str => "string".to_string(),
            ParamType::Choice(_) => "choice".to_string(),
            ParamType::Path(_) => "path".to_string(),
            ParamType::Image => "image".to_string(),
            ParamType::List(inner) => format!("list<{}>", inner.name()),
        }
    }

    /// JSON Schema describing values of this type as seen by tools: images
    /// are passed as integer ids and paths as strings.
    pub fn json_schema(&self) -> Json {
        match self {
            ParamType::Bool => json!({ "type": "boolean" }),
            ParamType::Int => json!({ "type": "integer" }),
            ParamType::Float => json!({ "type": "number" }),
            ParamType::Str => json!({ "type": "string" }),
            ParamType::Choice(choices) => json!({ "type": "string", "enum": choices }),
            ParamType::Path(_) => json!({ "type": "string", "format": "path" }),
            ParamType::Image => json!({ "type": "integer", "format": "image-id", "minimum": 0 }),
            ParamType::List(inner) => json!({ "type": "array", "items": inner.json_schema() }),
        }
    }
}

impl Value {
    /// Converts `self` to a value of type `ty`, applying the lossless
    /// conversions frontends rely on (integer to float, string to path or
    /// choice). Returns a description of the problem if it does not fit.
    pub fn coerce(self, ty: &ParamType) -> Result<Value, String> {
        match (self, ty) {
            (v @ Value::Bool(_), ParamType::Bool)
            | (v @ Value::Int(_), ParamType::Int)
            | (v @ Value::Float(_), ParamType::Float)
            | (v @ Value::Str(_), ParamType::Str)
            | (v @ Value::Path(_), ParamType::Path(_))
            | (v @ Value::Image(_), ParamType::Image) => Ok(v),
            (Value::Int(i), ParamType::Float) => Ok(Value::Float(i as f64)),
            (Value::Str(s), ParamType::Path(_)) => Ok(Value::Path(PathBuf::from(s))),
            (Value::Str(s), ParamType::Choice(choices)) => {
                if choices.contains(&s) {
                    Ok(Value::Str(s))
                } else {
                    Err(format!("`{s}` is not one of: {}", choices.join(", ")))
                }
            }
            (Value::List(items), ParamType::List(inner)) => items
                .into_iter()
                .map(|v| v.coerce(inner))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List),
            (v, ty) => Err(format!("expected {}, found {}", ty.name(), v.type_name())),
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Path(_) => "path",
            Value::Image(_) => "image",
            Value::List(_) => "list",
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            Value::Bool(b) => json!(b),
            Value::Int(i) => json!(i),
            Value::Float(f) => json!(f),
            Value::Str(s) => json!(s),
            Value::Path(p) => json!(p.to_string_lossy()),
            Value::Image(id) => json!(id.0),
            Value::List(items) => Json::Array(items.iter().map(Value::to_json).collect()),
        }
    }

    /// Reads a value of type `ty` from JSON.
    pub fn from_json(ty: &ParamType, json: &Json) -> Result<Value, String> {
        let mismatch = || format!("expected {}, found `{json}`", ty.name());
        match ty {
            ParamType::Bool => json.as_bool().map(Value::Bool).ok_or_else(mismatch),
            ParamType::Int => json.as_i64().map(Value::Int).ok_or_else(mismatch),
            ParamType::Float => json.as_f64().map(Value::Float).ok_or_else(mismatch),
            ParamType::Str | ParamType::Choice(_) => json
                .as_str()
                .ok_or_else(mismatch)
                .and_then(|s| Value::Str(s.to_string()).coerce(ty)),
            ParamType::Path(_) => json
                .as_str()
                .map(|s| Value::Path(PathBuf::from(s)))
                .ok_or_else(mismatch),
            ParamType::Image => json
                .as_u64()
                .map(|id| Value::Image(DatasetId(id)))
                .ok_or_else(mismatch),
            ParamType::List(inner) => json
                .as_array()
                .ok_or_else(mismatch)?
                .iter()
                .map(|item| Value::from_json(inner, item))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List),
        }
    }

    /// Parses a value of type `ty` from command-line text. List items are
    /// separated by commas.
    pub fn parse_text(ty: &ParamType, text: &str) -> Result<Value, String> {
        let mismatch = || format!("expected {}, found `{text}`", ty.name());
        match ty {
            ParamType::Bool => match text {
                "true" | "yes" | "1" => Ok(Value::Bool(true)),
                "false" | "no" | "0" => Ok(Value::Bool(false)),
                _ => Err(mismatch()),
            },
            ParamType::Int => text.parse().map(Value::Int).map_err(|_| mismatch()),
            ParamType::Float => text.parse().map(Value::Float).map_err(|_| mismatch()),
            ParamType::Str | ParamType::Choice(_) => Value::Str(text.to_string()).coerce(ty),
            ParamType::Path(_) => Ok(Value::Path(PathBuf::from(text))),
            ParamType::Image => text
                .parse()
                .map(|id| Value::Image(DatasetId(id)))
                .map_err(|_| mismatch()),
            ParamType::List(inner) => {
                if text.is_empty() {
                    return Ok(Value::List(Vec::new()));
                }
                text.split(',')
                    .map(|item| Value::parse_text(inner, item))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::List)
            }
        }
    }

    /// Inverse of [`parse_text`](Self::parse_text).
    pub fn to_text(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            // `{:?}` keeps a decimal point or exponent so the text reads
            // back as the same float.
            Value::Float(f) => format!("{f:?}"),
            Value::Str(s) => s.clone(),
            Value::Path(p) => p.to_string_lossy().into_owned(),
            Value::Image(id) => id.to_string(),
            Value::List(items) => items
                .iter()
                .map(Value::to_text)
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::Int(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Value {
        Value::Int(v.into())
    }
}

impl From<usize> for Value {
    fn from(v: usize) -> Value {
        Value::Int(v as i64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Float(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Str(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Str(v)
    }
}

impl From<PathBuf> for Value {
    fn from(v: PathBuf) -> Value {
        Value::Path(v)
    }
}

impl From<DatasetId> for Value {
    fn from(v: DatasetId) -> Value {
        Value::Image(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coercions() {
        assert_eq!(
            Value::Int(2).coerce(&ParamType::Float),
            Ok(Value::Float(2.0))
        );
        assert_eq!(
            Value::from("a.png").coerce(&ParamType::Path(PathMode::Open)),
            Ok(Value::Path("a.png".into()))
        );
        let choice = ParamType::Choice(vec!["a".into(), "b".into()]);
        assert!(Value::from("a").coerce(&choice).is_ok());
        assert!(Value::from("c").coerce(&choice).is_err());
        assert!(Value::Float(1.5).coerce(&ParamType::Int).is_err());
        assert!(Value::from("1").coerce(&ParamType::Int).is_err());
    }

    #[test]
    fn text_and_json_round_trip() {
        let cases = [
            (ParamType::Bool, Value::Bool(true)),
            (ParamType::Int, Value::Int(-7)),
            (ParamType::Float, Value::Float(2.0)),
            (ParamType::Float, Value::Float(1e-9)),
            (ParamType::Str, Value::from("hello world")),
            (
                ParamType::Path(PathMode::Save),
                Value::Path("out/a.tif".into()),
            ),
            (ParamType::Image, Value::Image(DatasetId(3))),
            (
                ParamType::List(Box::new(ParamType::Int)),
                Value::List(vec![Value::Int(1), Value::Int(2)]),
            ),
        ];
        for (ty, value) in cases {
            assert_eq!(
                Value::parse_text(&ty, &value.to_text()).as_ref(),
                Ok(&value)
            );
            assert_eq!(Value::from_json(&ty, &value.to_json()).as_ref(), Ok(&value));
        }
    }

    #[test]
    fn json_integers_are_accepted_as_floats() {
        assert_eq!(
            Value::from_json(&ParamType::Float, &json!(2)),
            Ok(Value::Float(2.0))
        );
        assert!(Value::from_json(&ParamType::Int, &json!(2.5)).is_err());
        assert!(Value::from_json(&ParamType::Image, &json!(-1)).is_err());
    }
}
