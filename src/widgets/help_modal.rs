use egui::{
    Align, Area, Color32, Context, Frame, Id, Key, Layout, Modal, RichText, ScrollArea, Separator,
    TextStyle, Vec2, Widget,
};
use log::debug;

use crate::{ScrollAreaStateExt, keymap_action::ACTION_KEYMAPS};

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

    pub fn show(&mut self, ctx: &Context, dimension: Vec2) {
        let margin = 24.0;
        let spacing = 10.0;
        Modal::new(Id::new("help_modal"))
            .backdrop_color(Color32::from_black_alpha(180))
            .frame(Frame::popup(&ctx.style()).inner_margin(spacing))
            .show(ctx, |ui| {
                let total_spacing = margin * 2.0 + spacing * 2.0;
                ui.set_width(dimension.x - total_spacing);

                let measure_area = Area::new("hidden_measure".into())
                    .constrain(false)
                    .fixed_pos(egui::pos2(-1_000_000.0, -1_000_000.0));

                let header = ui.vertical_centered(|ui| {
                    ui.heading("Keyboard Shortcuts");
                    Separator::default().spacing(spacing).ui(ui);
                });
                let header_height = header.response.rect.height();

                let footer_ui = |ui: &mut egui::Ui| {
                    ui.vertical_centered(|ui| {
                        Separator::default().spacing(spacing).ui(ui);
                        ui.label(RichText::new("Press Escape to close").weak());
                    });
                };
                let footer_height = measure_area
                    .clone()
                    .show(ui.ctx(), footer_ui)
                    .response
                    .rect
                    .height();

                ui.spacing_mut().item_spacing = egui::vec2(0.0, 8.0);
                let mut scroll_area = ScrollArea::vertical()
                    .auto_shrink(false)
                    .max_height(dimension.y - header_height - footer_height - total_spacing);
                if self.is_first_render {
                    scroll_area = scroll_area.vertical_scroll_offset(0.0);
                    if let Some(id) = self.scroll_area_id
                        && let Err(e) = egui::scroll_area::State::reset_velocity(ctx, id)
                    {
                        debug!("failed to reset help modal's scroll area velocity: {e}");
                    }
                }

                let scroll_area_output = scroll_area.show(ui, |ui| {
                    let delta = ui.input(|i| {
                        let mut y = 0.0;
                        if i.key_pressed(Key::ArrowDown) {
                            y -= 60.0;
                        }
                        if i.key_pressed(Key::ArrowUp) {
                            y += 60.0;
                        }
                        y
                    });
                    if delta != 0.0 {
                        ui.scroll_with_delta(egui::vec2(0.0, delta));
                    }

                    let gap = 12.0;
                    let width = ui.available_width() - gap;
                    let key_block_padding = egui::vec2(8.0, 4.0);

                    for (i, group) in ACTION_KEYMAPS.iter().enumerate() {
                        ui.vertical_centered(|ui| {
                            if i > 0 {
                                Separator::default().spacing(8.0).shrink(48.0).ui(ui);
                            }
                            ui.label(
                                RichText::new(format!("{} Mode", group.name))
                                    .size(TextStyle::Heading.resolve(ui.style()).size * 0.9),
                            );
                        });

                        for entry in &group.entries {
                            let key_str = entry
                                .keys
                                .iter()
                                .map(|k| k.to_string())
                                .collect::<Vec<_>>()
                                .join(" ");

                            ui.horizontal(|ui| {
                                let key_block = ui.vertical(|ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(width * 0.35, 0.0),
                                        Layout::top_down(Align::RIGHT),
                                        |ui| {
                                            Frame::NONE
                                                .fill(ui.visuals().code_bg_color)
                                                .corner_radius(4.0)
                                                .inner_margin(key_block_padding)
                                                // TODO: use monospace font
                                                .show(ui, |ui| ui.label(key_str))
                                        },
                                    )
                                });
                                let key_height = key_block.response.rect.height();

                                ui.add_space(gap);

                                let desc_ui = |ui: &mut egui::Ui| {
                                    ui.allocate_ui(egui::vec2(width * 0.65, 0.0), |ui| {
                                        ui.label(entry.description)
                                    })
                                };
                                let desc_height = measure_area
                                    .clone()
                                    .show(ui.ctx(), desc_ui)
                                    .response
                                    .rect
                                    .height();
                                ui.vertical(|ui| {
                                    ui.add_space((key_height - desc_height).max(0.0) / 2.0);
                                    desc_ui(ui);
                                });
                            });
                        }
                    }
                });
                self.scroll_area_id = Some(scroll_area_output.id);

                footer_ui(ui);
            });

        self.is_first_render = false;
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
