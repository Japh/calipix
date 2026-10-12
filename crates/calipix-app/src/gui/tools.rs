//! The tool strip: a narrow column at the left edge of the window with a
//! button for each selection tool.
//!
//! It stands outside the dockable workspace because the tool is the
//! application's, not one image's: it stays put whichever tab is in
//! front, and costs the images a sliver of width rather than any height.
//! Choosing a tool runs its command, so the buttons, the Edit > Tool
//! menu, the palette and the single-key shortcuts are one thing.
//!
//! The pictures on the buttons are drawn here with the painter; there is
//! no icon font or image to ship.

use calipix_command::Args;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};

use crate::state::{AppState, CanvasKey, Tool};

/// Width of the strip.
pub const WIDTH: f32 = 36.0;
/// Side of a tool's button.
const BUTTON: f32 = 28.0;

/// Draws the strip and asks for the command of a tool that was clicked.
pub fn strip(ui: &mut egui::Ui, state: &mut AppState) {
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.add_space(4.0);
        for tool in Tool::ALL {
            if button(ui, tool, state.tool() == tool).clicked() {
                state.request(&tool.command_id(), Args::new());
            }
            // The pointer stands apart from the tools that draw.
            if tool == Tool::Pointer {
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);
            }
        }
    });
}

fn button(ui: &mut egui::Ui, tool: Tool, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON, BUTTON), Sense::click());
    let name = format!("{} Tool", tool.title());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, active, name.as_str())
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter();
        // The active tool is marked by its plate and by a bar beside it,
        // not by colour alone.
        if active {
            painter.rect_filled(rect, 4.0, visuals.selection.bg_fill);
            let bar = Rect::from_min_max(
                pos2(rect.left() - 4.0, rect.top() + 5.0),
                pos2(rect.left() - 2.0, rect.bottom() - 5.0),
            );
            painter.rect_filled(bar, 1.0, visuals.selection.stroke.color);
        } else if response.hovered() {
            painter.rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let color = if active {
            visuals.selection.stroke.color
        } else if response.hovered() {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        };
        painter.extend(glyph(tool, rect.center(), color));
    }
    response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(format!("{}   {}", tool.title(), tool.shortcut())).strong());
        ui.label(tool.hint());
    })
}

/// The picture on a tool's button, about sixteen points across, centred
/// on `c`.
fn glyph(tool: Tool, c: Pos2, color: Color32) -> Vec<Shape> {
    let line = Stroke::new(1.5, color);
    let at = |x: f32, y: f32| pos2(c.x + x, c.y + y);
    let knob = |x: f32, y: f32| {
        Shape::rect_filled(Rect::from_center_size(at(x, y), vec2(3.5, 3.5)), 0.0, color)
    };
    match tool {
        // An arrow, as the pointer of the desktop.
        Tool::Pointer => {
            let outline = vec![
                at(-4.5, -7.0),
                at(-4.5, 5.5),
                at(-1.2, 2.6),
                at(1.2, 7.5),
                at(3.2, 6.5),
                at(0.9, 1.8),
                at(5.0, 1.8),
            ];
            vec![Shape::closed_line(outline, line)]
        }
        Tool::Rectangle => vec![Shape::rect_stroke(
            Rect::from_center_size(c, vec2(15.0, 11.0)),
            0.0,
            line,
            egui::StrokeKind::Middle,
        )],
        Tool::Oval => vec![Shape::ellipse_stroke(c, vec2(7.5, 5.5), line)],
        Tool::Polygon => vec![Shape::closed_line(
            vec![
                at(-6.5, -2.5),
                at(-1.0, -7.0),
                at(7.0, -3.5),
                at(4.5, 6.5),
                at(-5.0, 5.5),
            ],
            line,
        )],
        // A closed outline that is plainly not made of straight edges.
        Tool::Freehand => {
            let points = (0..40)
                .map(|step| {
                    let angle = std::f32::consts::TAU * step as f32 / 40.0;
                    let radius = 5.6 + 1.5 * (3.0 * angle + 0.6).sin();
                    at(1.25 * radius * angle.cos(), radius * angle.sin())
                })
                .collect();
            vec![Shape::closed_line(points, line)]
        }
        Tool::Line => vec![
            Shape::line_segment([at(-6.5, 6.0), at(6.5, -6.0)], line),
            knob(-6.5, 6.0),
            knob(6.5, -6.0),
        ],
        Tool::Polyline => vec![
            Shape::line(
                vec![at(-7.0, 5.5), at(-2.5, -5.5), at(2.5, 3.0), at(7.0, -5.0)],
                line,
            ),
            knob(-7.0, 5.5),
            knob(-2.5, -5.5),
            knob(2.5, 3.0),
            knob(7.0, -5.0),
        ],
        // The marker a point is drawn as.
        Tool::Point => vec![
            Shape::line_segment([at(-7.0, 0.0), at(-2.5, 0.0)], line),
            Shape::line_segment([at(2.5, 0.0), at(7.0, 0.0)], line),
            Shape::line_segment([at(0.0, -7.0), at(0.0, -2.5)], line),
            Shape::line_segment([at(0.0, 2.5), at(0.0, 7.0)], line),
            Shape::circle_filled(c, 1.8, color),
        ],
    }
}

/// The keys that act on the selection of the active image: arrows nudge
/// it (with Alt they resize a rectangle or oval), Escape gives up a shape
/// being drawn, Enter finishes a polygon, Backspace takes back its last
/// vertex, and Delete removes the regions picked in the list. A key that
/// has nothing to act on is left for the shortcuts.
///
/// Call after the keys of the Results table and the Regions list, which
/// come first while they have the keyboard, and before shortcuts are
/// looked up.
pub fn keys(ctx: &egui::Context, state: &mut AppState) {
    if ctx.text_edit_focused() || state.table_has_attention() {
        return;
    }
    use egui::{Key, Modifiers};
    let arrow = |dx, dy, resize| CanvasKey::Arrow { dx, dy, resize };
    let bindings = [
        (Modifiers::NONE, Key::Escape, CanvasKey::Escape),
        (Modifiers::NONE, Key::Enter, CanvasKey::Enter),
        (Modifiers::NONE, Key::Backspace, CanvasKey::Backspace),
        (Modifiers::NONE, Key::Delete, CanvasKey::Delete),
        (Modifiers::NONE, Key::ArrowLeft, arrow(-1, 0, false)),
        (Modifiers::NONE, Key::ArrowRight, arrow(1, 0, false)),
        (Modifiers::NONE, Key::ArrowUp, arrow(0, -1, false)),
        (Modifiers::NONE, Key::ArrowDown, arrow(0, 1, false)),
        (Modifiers::ALT, Key::ArrowLeft, arrow(-1, 0, true)),
        (Modifiers::ALT, Key::ArrowRight, arrow(1, 0, true)),
        (Modifiers::ALT, Key::ArrowUp, arrow(0, -1, true)),
        (Modifiers::ALT, Key::ArrowDown, arrow(0, 1, true)),
    ];
    for (modifiers, key, action) in bindings {
        let presses = ctx.input(|input| {
            input
                .events
                .iter()
                .filter(|event| {
                    matches!(event, egui::Event::Key { key: k, pressed: true, modifiers: m, .. }
                        if *k == key && m.matches_exact(modifiers))
                })
                .count()
        });
        // A held arrow repeats, and each repeat is a nudge.
        let mut used = false;
        for _ in 0..presses {
            used |= state.canvas_key(action);
        }
        if used {
            ctx.input_mut(|input| {
                input.events.retain(|event| {
                    !matches!(event, egui::Event::Key { key: k, pressed: true, modifiers: m, .. }
                        if *k == key && m.matches_exact(modifiers))
                });
            });
        }
    }
}
