//! Checks that every command in a registry is fit to be shown in each
//! frontend. Run it in a test against the full registry so that no command
//! can ship with a definition that some frontend cannot use.

use std::collections::BTreeMap;

use crate::{CommandSpec, Registry, Value};

/// Returns a description of every problem found; empty means conformant.
pub fn check(registry: &Registry) -> Vec<String> {
    let mut problems = Vec::new();
    let mut ijm_names: BTreeMap<&str, &str> = BTreeMap::new();
    let mut menu_items: BTreeMap<(Vec<String>, &str), &str> = BTreeMap::new();

    for spec in registry.specs() {
        let mut report = |problem: String| problems.push(format!("{}: {problem}", spec.id));

        if let Err(e) = spec.validate() {
            report(e.to_string());
        }
        check_one(spec, &mut report);

        if let Some(name) = spec.ijm_name.as_deref()
            && let Some(other) = ijm_names.insert(name, &spec.id)
        {
            report(format!("ImageJ name `{name}` is also used by {other}"));
        }
        if let Some(menu) = &spec.menu {
            let key = (menu.path.clone(), spec.title.as_str());
            if let Some(other) = menu_items.insert(key, &spec.id) {
                report(format!(
                    "menu item `{} > {}` is also used by {other}",
                    menu.path.join(" > "),
                    spec.title
                ));
            }
        }
    }
    problems
}

fn check_one(spec: &CommandSpec, report: &mut impl FnMut(String)) {
    if spec.summary.trim().is_empty() {
        report("summary is empty; it is shown in the palette and to agents".to_string());
    }
    if spec.title != spec.title.trim() {
        report("title has surrounding whitespace".to_string());
    }

    let schema = spec.input_schema();
    if schema["type"] != "object" || !schema["properties"].is_object() {
        report("input schema is not an object schema".to_string());
    }

    for param in &spec.inputs {
        if param.label.trim().is_empty() {
            report(format!("parameter `{}` has no label", param.name));
        }
        // A default must survive the trip through each textual frontend,
        // or dialogs, the command line and tools would disagree about it.
        if let Some(default) = &param.default {
            let via_text = Value::parse_text(&param.ty, &default.to_text());
            if via_text.as_ref() != Ok(default) {
                report(format!(
                    "default of `{}` does not round-trip through text: {via_text:?}",
                    param.name
                ));
            }
            let via_json = Value::from_json(&param.ty, &default.to_json());
            if via_json.as_ref() != Ok(default) {
                report(format!(
                    "default of `{}` does not round-trip through JSON: {via_json:?}",
                    param.name
                ));
            }
        }
        if schema["properties"][&param.name].is_null() {
            report(format!(
                "parameter `{}` missing from input schema",
                param.name
            ));
        }
    }
}
