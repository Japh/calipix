use crate::Registry;

/// One entry of the application menu, built from command specs.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Submenu { title: String, items: Vec<MenuItem> },
    Command { id: String, title: String },
}

struct Node {
    title: String,
    /// Smallest weight of any command beneath this node; orders it among
    /// its siblings.
    weight: f64,
    kind: Kind,
}

enum Kind {
    Submenu(Vec<Node>),
    Command(String),
}

/// Builds the menu from every command that declares a menu path. Within a
/// menu, entries are ordered by weight and then by title; a submenu takes
/// the lowest weight of the commands inside it.
pub fn menu_tree(registry: &Registry) -> Vec<MenuItem> {
    let mut root: Vec<Node> = Vec::new();
    for spec in registry.specs() {
        let Some(menu) = &spec.menu else { continue };
        let mut level = &mut root;
        for part in &menu.path {
            let index = level
                .iter()
                .position(|n| matches!(n.kind, Kind::Submenu(_)) && &n.title == part)
                .unwrap_or_else(|| {
                    level.push(Node {
                        title: part.clone(),
                        weight: f64::INFINITY,
                        kind: Kind::Submenu(Vec::new()),
                    });
                    level.len() - 1
                });
            let node = &mut level[index];
            node.weight = node.weight.min(menu.weight);
            let Kind::Submenu(children) = &mut node.kind else {
                unreachable!("position() only matches submenus");
            };
            level = children;
        }
        level.push(Node {
            title: spec.title.clone(),
            weight: menu.weight,
            kind: Kind::Command(spec.id.clone()),
        });
    }
    finish(root)
}

fn finish(mut nodes: Vec<Node>) -> Vec<MenuItem> {
    nodes.sort_by(|a, b| {
        a.weight
            .total_cmp(&b.weight)
            .then_with(|| a.title.cmp(&b.title))
    });
    nodes
        .into_iter()
        .map(|node| match node.kind {
            Kind::Submenu(children) => MenuItem::Submenu {
                title: node.title,
                items: finish(children),
            },
            Kind::Command(id) => MenuItem::Command {
                id,
                title: node.title,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Args, Command, CommandCtx, CommandSpec, Outputs, Result};
    use std::sync::Arc;

    struct Stub(CommandSpec);

    impl Command for Stub {
        fn spec(&self) -> &CommandSpec {
            &self.0
        }

        fn run(&self, _: &mut CommandCtx<'_>, _: &Args) -> Result<Outputs> {
            Ok(Outputs::new())
        }
    }

    fn command(id: &str, title: &str) -> MenuItem {
        MenuItem::Command {
            id: id.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn groups_and_orders_by_weight_then_title() {
        let mut registry = Registry::new();
        let mut add = |id: &str, title: &str, menu: Option<(&str, f64)>| {
            let mut spec = CommandSpec::new(id, title);
            if let Some((path, weight)) = menu {
                spec = spec.menu(path, weight);
            }
            registry.register(Arc::new(Stub(spec))).unwrap();
        };
        add("a.sharpen", "Sharpen", Some(("Process > Filters", 20.0)));
        add("a.blur", "Blur", Some(("Process > Filters", 20.0)));
        add("a.math", "Add", Some(("Process", 30.0)));
        add("a.open", "Open", Some(("File", 1.0)));
        add("a.fft", "FFT", Some(("Process", 5.0)));
        add("a.hidden", "Hidden", None);

        assert_eq!(
            menu_tree(&registry),
            [
                MenuItem::Submenu {
                    title: "File".into(),
                    items: vec![command("a.open", "Open")],
                },
                MenuItem::Submenu {
                    title: "Process".into(),
                    items: vec![
                        command("a.fft", "FFT"),
                        MenuItem::Submenu {
                            title: "Filters".into(),
                            items: vec![command("a.blur", "Blur"), command("a.sharpen", "Sharpen")],
                        },
                        command("a.math", "Add"),
                    ],
                },
            ]
        );
    }
}
