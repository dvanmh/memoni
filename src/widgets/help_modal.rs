use egui::{
    Color32, Frame, Id, Key, Label, Modal, Rect, RichText, ScrollArea, Separator, TextStyle, Widget,
};
use log::debug;

use crate::{
    ScrollAreaStateExt,
    keymap_spec::{KEYMAP_SPECS, KeymapEntry},
};

pub struct HelpModal {
    scroll_area_id: Option<egui::Id>,
    is_first_render: bool,
}

impl HelpModal {
    pub fn new() -> Self {
        HelpModal {
            scroll_area_id: None,
            is_first_render: true,
        }
    }

    pub fn show(&mut self, ui: &egui::Ui) {
        let margin = 10.0;
        let spacing = 8.0;

        Modal::new(Id::new("help_modal"))
            .backdrop_color(Color32::from_black_alpha(180))
            .frame(Frame::popup(&ui.global_style()).inner_margin(0.0))
            .show(ui, |ui| {
                let rect = ui.content_rect();
                ui.set_width(rect.width() - margin * 2.0);
                ui.set_height(rect.height() - margin * 2.0);

                ui.vertical_centered(|ui| {
                    ui.add_space(spacing);
                    ui.heading("Keyboard Shortcuts");
                    ui.add_space(spacing);
                    Separator::default().spacing(0.0).shrink(spacing).ui(ui);
                });

                let footer_ui = |ui: &mut egui::Ui| {
                    ui.vertical_centered(|ui| {
                        Separator::default().spacing(0.0).shrink(spacing).ui(ui);
                        ui.add_space(spacing);
                        ui.label(RichText::new("Press Escape to close").weak());
                        ui.add_space(spacing);
                    });
                };
                let footer_height = measure_ui(ui, footer_ui).height();

                let mut scroll_area = ScrollArea::vertical()
                    .auto_shrink(false)
                    .max_height(ui.available_height() - footer_height);
                if self.is_first_render {
                    scroll_area = scroll_area.vertical_scroll_offset(0.0);
                    if let Some(id) = self.scroll_area_id
                        && let Err(e) = egui::scroll_area::State::reset_velocity(ui, id)
                    {
                        debug!("failed to reset help modal's scroll area velocity: {e}");
                    }
                }

                let scroll_area_output = scroll_area.show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::vec2(spacing, 6.0))
                        .show(ui, |ui| {
                            let delta = ui.input(|i| {
                                let mut y = 0.0;
                                if i.key_pressed(Key::ArrowDown) {
                                    y -= 100.0;
                                }
                                if i.key_pressed(Key::ArrowUp) {
                                    y += 100.0;
                                }
                                y
                            });
                            if delta != 0.0 {
                                ui.scroll_with_delta(egui::vec2(0.0, delta));
                            }

                            for (i, spec) in KEYMAP_SPECS.iter().enumerate() {
                                ui.vertical_centered(|ui| {
                                    if i > 0 {
                                        ui.add_space(4.0);
                                        Separator::default().spacing(8.0).shrink(48.0).ui(ui);
                                    }
                                    ui.label(
                                        RichText::new(format!("{} Mode", spec.name)).size(
                                            TextStyle::Heading.resolve(ui.style()).size * 0.9,
                                        ),
                                    );
                                });

                                for (j, entry) in spec.entries.iter().enumerate() {
                                    if j > 0 {
                                        ui.add_space(2.0);
                                    }

                                    Self::render_entry(ui, entry);
                                }
                            }
                        });
                });
                self.scroll_area_id = Some(scroll_area_output.id);

                footer_ui(ui);
            });

        self.is_first_render = false;
    }

    fn render_entry(ui: &mut egui::Ui, entry: &KeymapEntry) {
        let col_gap = 12.0;
        let binding_gap = 4.0;
        let binding_col_width = ui.available_width() * 0.35;
        let desc_col_width = ui.available_width() - col_gap - binding_col_width;

        // TODO: use monospace font
        let frame = Frame::NONE
            .fill(ui.visuals().code_bg_color)
            .corner_radius(4.0)
            .inner_margin(egui::vec2(8.0, 4.0));

        let mut binding_cursor = Rect::from_min_size(ui.cursor().min, egui::vec2(0.0, 0.0));
        let start_y = binding_cursor.min.y;
        let min_x = binding_cursor.min.x;
        let max_binding_x = min_x + binding_col_width;
        let mut binding_rects = vec![];
        for binding in entry.bindings {
            let rect = measure_ui(ui, |ui| {
                frame.show(ui, |ui| ui.label(*binding));
            });

            if binding_cursor.min.x + rect.width() > max_binding_x {
                binding_cursor = binding_cursor
                    .translate(egui::vec2(0.0, binding_cursor.height() + binding_gap))
                    .with_min_x(min_x)
                    .with_max_x(min_x);
            }

            // TODO: use the largest binding frame width as binding_col_width
            if binding_cursor.min.x == min_x && rect.width() > binding_col_width {
                binding_rects.push(Rect::from_min_size(binding_cursor.min, rect.size()));
                binding_cursor = Rect::from_min_size(
                    binding_cursor
                        .translate(egui::vec2(0.0, rect.height() + binding_gap))
                        .min,
                    egui::vec2(0.0, 0.0),
                );
                continue;
            }

            binding_rects.push(Rect::from_min_size(binding_cursor.min, rect.size()));
            binding_cursor = Rect::from_min_size(
                binding_cursor
                    .translate(egui::vec2(rect.width() + binding_gap, 0.0))
                    .min,
                egui::vec2(0.0, binding_cursor.height().max(rect.height())),
            );
        }

        let desc_height = measure_ui_in_rect(
            ui,
            Rect::from_min_size(ui.cursor().min, egui::vec2(desc_col_width, f32::INFINITY)),
            |ui| {
                ui.label(entry.description);
            },
        )
        .height();

        let end_y = binding_cursor.max.y.max(start_y + desc_height);
        let binding_padding = if binding_cursor.max.y < end_y {
            (end_y - binding_cursor.max.y) / 2.0
        } else {
            0.0
        };

        for (&rect, binding) in binding_rects.iter().zip(entry.bindings) {
            let rect = if binding_padding > 0.0 {
                rect.translate(egui::vec2(0.0, binding_padding))
            } else {
                rect
            };

            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                frame.show(ui, |ui| ui.label(*binding))
            });
            ui.advance_cursor_after_rect(rect);
        }

        ui.painter().vline(
            max_binding_x + col_gap / 2.0,
            (start_y + 2.0)..=(end_y - 2.0),
            ui.style().visuals.noninteractive().bg_stroke,
        );

        let min_desc_x = max_binding_x + col_gap;
        let max_x = max_binding_x + col_gap + desc_col_width;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_max(
                egui::pos2(min_desc_x, start_y),
                egui::pos2(max_x, end_y),
            )),
            |ui| ui.horizontal_centered(|ui| ui.add(Label::new(entry.description).wrap())),
        );
    }

    pub fn hide(&mut self) {
        self.is_first_render = true;
    }
}

impl Default for HelpModal {
    fn default() -> Self {
        Self::new()
    }
}

fn measure_ui(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) -> Rect {
    measure_ui_in_rect(ui, ui.max_rect(), add_contents)
}

fn measure_ui_in_rect(
    ui: &mut egui::Ui,
    rect: Rect,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> Rect {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .sizing_pass()
            .invisible()
            .max_rect(rect),
    );
    add_contents(&mut child);
    child.min_rect()
}
