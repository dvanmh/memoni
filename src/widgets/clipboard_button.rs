use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontSelection, Galley, Image, Pos2, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, TextStyle, TextWrapMode, TextureHandle, Ui, Vec2, Widget,
    WidgetText, text::LayoutJob,
};

#[derive(Default, Clone)]
pub struct ClipboardButton {
    labels: Vec<Vec<RichText>>,
    sublabel: Option<WidgetText>,
    secondary_foreground: Option<Color32>,
    preview: Option<(TextureHandle, Vec2)>,
    preview_source: Option<String>,
    preview_background: Color32,
    is_active: bool,
    with_preview_padding: Option<Vec2>,
    underline_offset: f32,
    is_pinned: bool,
    pin_size: f32,
    pin_color: Color32,
    keyboard_hint: Option<String>,
    color_preview: Option<Color32>,
    color_preview_size: f32,
    color_preview_corner_radius: u8,
    color_preview_background: Option<TextureHandle>,
}

impl ClipboardButton {
    #[inline]
    pub fn label(mut self, label: Vec<RichText>) -> Self {
        self.labels = vec![label];
        self
    }

    #[inline]
    pub fn append_label(mut self, label: Vec<RichText>) -> Self {
        self.labels.push(label);
        self
    }

    #[inline]
    pub fn sublabel(mut self, sublabel: impl Into<WidgetText>) -> Self {
        self.sublabel = Some(sublabel.into());
        self
    }

    #[inline]
    pub fn secondary_foreground(mut self, secondary_foreground: impl Into<Color32>) -> Self {
        self.secondary_foreground = Some(secondary_foreground.into());
        self
    }

    #[inline]
    pub fn preview(mut self, texture: TextureHandle, size: impl Into<Vec2>) -> Self {
        self.preview = Some((texture, size.into()));
        self
    }

    #[inline]
    pub fn preview_source(mut self, preview_source: &str) -> Self {
        self.preview_source = Some(preview_source.to_string());
        self
    }

    #[inline]
    pub fn preview_background(mut self, preview_background: impl Into<Color32>) -> Self {
        self.preview_background = preview_background.into();
        self
    }

    #[inline]
    pub fn is_active(mut self, is_active: bool) -> Self {
        self.is_active = is_active;
        self
    }

    #[inline]
    pub fn with_preview_padding(mut self, with_preview_padding: impl Into<Vec2>) -> Self {
        self.with_preview_padding = Some(with_preview_padding.into());
        self
    }

    #[inline]
    pub fn underline_offset(mut self, underline_offset: f32) -> Self {
        self.underline_offset = underline_offset;
        self
    }

    #[inline]
    pub fn is_pinned(mut self, is_pinned: bool) -> Self {
        self.is_pinned = is_pinned;
        self
    }

    #[inline]
    pub fn pin_size(mut self, pin_size: f32) -> Self {
        self.pin_size = pin_size;
        self
    }

    #[inline]
    pub fn pin_color(mut self, pin_color: impl Into<Color32>) -> Self {
        self.pin_color = pin_color.into();
        self
    }

    #[inline]
    pub fn keyboard_hint(mut self, keyboard_hint: impl Into<String>) -> Self {
        self.keyboard_hint = Some(keyboard_hint.into());
        self
    }

    #[inline]
    pub fn color_preview(mut self, color_preview: impl Into<Color32>) -> Self {
        self.color_preview = Some(color_preview.into());
        self
    }

    #[inline]
    pub fn color_preview_size(mut self, color_preview_size: f32) -> Self {
        self.color_preview_size = color_preview_size;
        self
    }

    #[inline]
    pub fn color_preview_corner_radius(mut self, color_preview_corner_radius: u8) -> Self {
        self.color_preview_corner_radius = color_preview_corner_radius;
        self
    }

    #[inline]
    pub fn color_preview_background(mut self, color_preview_background: TextureHandle) -> Self {
        self.color_preview_background = Some(color_preview_background);
        self
    }
}

impl Widget for ClipboardButton {
    fn ui(self, ui: &mut Ui) -> Response {
        // TODO: make these configurable?
        let sublabel_gap = 3.0;
        let keyboard_hint_gap = 10.0;
        let keyboard_hint_size = 11.0;

        let padding = if self.preview.is_some()
            && let Some(with_preview_padding) = self.with_preview_padding
        {
            with_preview_padding
        } else {
            ui.style().spacing.button_padding
        };
        let right_padding = ui.style().spacing.button_padding.x;

        let desired_width = ui.available_width();

        let mut text_width = desired_width - padding.x - right_padding;
        if let Some((_, img_size)) = self.preview {
            text_width -= img_size.x;
        }
        if self.color_preview.is_some() {
            text_width -= self.color_preview_size + padding.x;
        }

        let keyboard_hint_galley = self.keyboard_hint.map(|sh| {
            WidgetText::RichText(Arc::new(
                RichText::new(sh)
                    .size(keyboard_hint_size)
                    .color(self.secondary_foreground.unwrap_or(Color32::PLACEHOLDER)),
            ))
            .into_galley(
                ui,
                Some(TextWrapMode::Truncate),
                desired_width * 0.2,
                TextStyle::Button,
            )
        });
        text_width -= keyboard_hint_galley
            .as_ref()
            .map(|g| g.size().x + keyboard_hint_gap)
            .unwrap_or(0.0);

        let galleys = self
            .labels
            .into_iter()
            .map(|l| {
                rich_texts_to_galley(
                    l,
                    ui,
                    Some(TextWrapMode::Truncate),
                    text_width,
                    TextStyle::Button,
                )
            })
            .collect::<Vec<_>>();
        let sublabel_galley = self.sublabel.map(|sl| {
            sl.color(self.secondary_foreground.unwrap_or(Color32::PLACEHOLDER))
                .into_galley(
                    ui,
                    Some(TextWrapMode::Truncate),
                    text_width,
                    TextStyle::Button,
                )
        });
        let img_src_galley = if self.preview.is_some() {
            self.preview_source.as_ref().map(|s| {
                Into::<WidgetText>::into(s).into_galley(
                    ui,
                    Some(TextWrapMode::Truncate),
                    text_width,
                    TextStyle::Button,
                )
            })
        } else {
            None
        };

        let text_height = galleys.iter().fold(0.0, |acc, g| acc + g.size().y)
            + sublabel_galley
                .as_ref()
                .map(|g| g.size().y + sublabel_gap)
                .unwrap_or(0.0)
            + img_src_galley.as_ref().map(|g| g.size().y).unwrap_or(0.0);
        let preview_height = self.preview.as_ref().map(|i| i.1.y).unwrap_or(0.0);
        let color_preview_height = self
            .color_preview
            .as_ref()
            .map(|_| self.color_preview_size + padding.y * 2.0)
            .unwrap_or(0.0);
        let desired_height = preview_height
            .max(color_preview_height)
            .max(text_height + padding.y * 2.0);

        let (rect, response) =
            ui.allocate_at_least(Vec2::new(desired_width, desired_height), Sense::CLICK);

        if ui.is_rect_visible(rect) {
            let visuals = &ui.style().visuals.widgets.inactive;
            let bg_fill = if self.is_active {
                ui.style().visuals.widgets.active.weak_bg_fill
            } else {
                visuals.weak_bg_fill
            };

            ui.painter().rect(
                rect,
                visuals.corner_radius,
                bg_fill,
                Stroke::NONE,
                StrokeKind::Inside,
            );

            let mut cursor_x = rect.min.x;
            if let Some((ref texture, size)) = self.preview {
                let preview_rect =
                    Rect::from_min_size(rect.min, egui::vec2(size.x, desired_height));
                let preview = Image::from_texture(texture)
                    .maintain_aspect_ratio(true)
                    .bg_fill(self.preview_background)
                    .corner_radius(CornerRadius {
                        nw: visuals.corner_radius.nw,
                        sw: visuals.corner_radius.sw,
                        ..Default::default()
                    });

                preview.paint_at(ui, preview_rect);

                assert!(preview_rect.width() == size.x);
                cursor_x += preview_rect.width();
            }

            if let Some(color_preview) = self.color_preview {
                cursor_x += padding.x;

                let preview_rect = Rect::from_center_size(
                    egui::pos2(cursor_x + self.color_preview_size / 2.0, rect.center().y),
                    Vec2::splat(self.color_preview_size),
                );

                if let Some(background_texture) = self.color_preview_background {
                    let preview_background = Image::from_texture(&background_texture)
                        .maintain_aspect_ratio(true)
                        .corner_radius(CornerRadius::same(self.color_preview_corner_radius));
                    preview_background.paint_at(ui, preview_rect);
                }

                ui.painter().rect_filled(
                    preview_rect,
                    CornerRadius::same(self.color_preview_corner_radius),
                    color_preview,
                );

                ui.painter().rect_stroke(
                    preview_rect,
                    CornerRadius::same(self.color_preview_corner_radius),
                    Stroke::new(1.2_f32, visuals.fg_stroke.color),
                    StrokeKind::Outside,
                );

                cursor_x += preview_rect.width();
            }

            cursor_x += padding.x;
            let mut cursor_y = rect.min.y + padding.y;
            for galley in galleys {
                let text_pos = Pos2::new(cursor_x, cursor_y);
                cursor_y += galley.size().y;
                ui.painter().galley(text_pos, galley, visuals.text_color());
            }

            if let Some(galley) = img_src_galley {
                let text_pos = Pos2::new(cursor_x, cursor_y);
                let text_underline = Stroke {
                    width: 1.0,
                    color: visuals.text_color(),
                };

                // Drawing text underline manually with offset to workaround https://github.com/emilk/egui/issues/5855
                let underline_y =
                    text_pos.y + galley.size().y - text_underline.width + self.underline_offset;
                ui.painter().line_segment(
                    [
                        Pos2::new(text_pos.x, underline_y),
                        Pos2::new(text_pos.x + galley.size().x, underline_y),
                    ],
                    text_underline,
                );
                ui.painter().galley(text_pos, galley, visuals.text_color());
            }

            if let Some(galley) = sublabel_galley {
                let text_pos =
                    Pos2::new(cursor_x, rect.shrink2(padding).bottom() - galley.size().y);
                ui.painter().galley(
                    text_pos,
                    galley,
                    self.secondary_foreground.unwrap_or(visuals.text_color()),
                );
            }

            if let Some(galley) = keyboard_hint_galley {
                cursor_x += text_width + keyboard_hint_gap;
                let text_pos = Pos2::new(cursor_x, rect.center().y - galley.rect.height() / 2.0);
                ui.painter().galley(
                    text_pos,
                    galley,
                    self.secondary_foreground.unwrap_or(visuals.text_color()),
                );
            }

            if self.is_pinned {
                let pin_center = rect.min + Vec2::splat(self.pin_size / 2.0);
                ui.painter()
                    .circle_filled(pin_center, self.pin_size, self.pin_color);
            }
        }

        response
    }
}

fn rich_texts_to_galley(
    rich_texts: Vec<RichText>,
    ui: &Ui,
    wrap_mode: Option<TextWrapMode>,
    available_width: f32,
    fallback_font: impl Into<FontSelection>,
) -> Arc<Galley> {
    let valign = ui.text_valign();
    let style = ui.style();
    let fallback_font = fallback_font.into();

    let mut layout_job = LayoutJob::default();
    for text in rich_texts {
        text.append_to(&mut layout_job, style, fallback_font.clone(), valign);
    }

    WidgetText::from(layout_job).into_galley(ui, wrap_mode, available_width, fallback_font)
}
