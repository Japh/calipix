//! The command palette: a search box over the window listing every command.

use eframe::egui::{self, Key, Modifiers};
use imager_command::CommandSpec;

use super::ui;
use crate::state::AppState;

#[derive(Debug, Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    /// Index of the highlighted result.
    pub selected: usize,
}

const WIDTH: f32 = 520.0;
const ROW_HEIGHT: f32 = 40.0;

impl Palette {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    /// Draws the palette if it is open. Returns the id of the command the
    /// person chose. Call before the rest of the interface so that the
    /// palette sees Enter, Esc and the arrow keys first.
    pub fn show(&mut self, ctx: &egui::Context, state: &AppState) -> Option<String> {
        if !self.open {
            return None;
        }
        let hits = state.search(&self.query);
        // Taken before the search box can treat them as text editing.
        let (enter, down, up) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
            )
        });
        if !hits.is_empty() {
            let last = hits.len() - 1;
            self.selected = self.selected.min(last);
            if down {
                self.selected = if self.selected == last {
                    0
                } else {
                    self.selected + 1
                };
            }
            if up {
                self.selected = if self.selected == 0 {
                    last
                } else {
                    self.selected - 1
                };
            }
        }
        let mut chosen = enter
            .then(|| hits.get(self.selected).map(|spec| spec.id.clone()))
            .flatten();

        let id = egui::Id::new("command_palette");
        let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, [0.0, 72.0]);
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(10);
        let palette = egui::Modal::new(id).area(area).frame(frame);
        let modal = palette.show(ctx, |ui| {
            ui.set_width(WIDTH);
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search commands by name")
                    .font(egui::TextStyle::Heading)
                    .margin(egui::vec2(8.0, 6.0))
                    .desired_width(f32::INFINITY),
            );
            if search.changed() {
                self.selected = 0;
            }
            search.request_focus();
            ui.add_space(2.0);
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(ROW_HEIGHT * 8.5)
                .show(ui, |ui| {
                    if hits.is_empty() {
                        ui.add_space(ui::GAP);
                        ui::weak(ui, format!("No command matches “{}”", self.query.trim()));
                        ui.add_space(ui::GAP);
                    }
                    for (index, spec) in hits.iter().enumerate() {
                        let selected = index == self.selected;
                        let response = row(ui, state, spec, selected);
                        if selected && (down || up) {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            chosen = Some(spec.id.clone());
                        }
                    }
                });
        });
        if modal.should_close() || chosen.is_some() {
            self.open = false;
        }
        chosen
    }
}

/// One result: title and shortcut on the first line; below, where the
/// command is in the menus and what it does, or why it cannot run now.
fn row(ui: &mut egui::Ui, state: &AppState, spec: &CommandSpec, selected: bool) -> egui::Response {
    let unavailable = state.unavailable_reason(&spec.id);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            unavailable.is_none(),
            selected,
            &spec.title,
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    if selected {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    // Unavailable commands are dimmed and say why; the wording, not the
    // colour, carries the meaning.
    let (strong, weak) = match (&unavailable, selected) {
        // Dimmed text would be unreadable on the selection colour.
        (Some(_), true) => (visuals.text_color(), visuals.text_color()),
        (Some(_), false) => (visuals.weak_text_color(), visuals.weak_text_color()),
        (None, true) => (
            visuals.selection.stroke.color,
            visuals.selection.stroke.color,
        ),
        (None, false) => (visuals.strong_text_color(), visuals.text_color()),
    };
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let inner = rect.shrink2(egui::vec2(8.0, 4.0));
    let painter = ui.painter().with_clip_rect(inner);
    let mut title_right = inner.right();
    if let Some(shortcut) = state.shortcut_text(&spec.id) {
        let drawn = painter.text(
            inner.right_top(),
            egui::Align2::RIGHT_TOP,
            shortcut,
            small.clone(),
            weak,
        );
        title_right = drawn.left() - ui::GAP;
    }
    painter
        .with_clip_rect(egui::Rect::from_min_max(
            inner.min,
            egui::pos2(title_right, inner.bottom()),
        ))
        .text(
            inner.left_top(),
            egui::Align2::LEFT_TOP,
            &spec.title,
            body,
            strong,
        );

    let place = spec.menu.as_ref().map(|menu| menu.path.join(" › "));
    let detail = match (&unavailable, place) {
        (Some(reason), _) => format!("Unavailable: {reason}"),
        (None, Some(place)) => format!("{place}  ·  {}", spec.summary),
        (None, None) => spec.summary.clone(),
    };
    painter.text(
        inner.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        detail,
        small,
        weak,
    );
    response
}
