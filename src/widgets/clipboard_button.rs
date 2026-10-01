use std::{iter::Peekable, mem, range::Range, slice::Iter, sync::Arc};

use egui::{
    Color32, CornerRadius, FontId, Galley, Image, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, TextFormat, TextStyle, TextWrapMode, TextureHandle, Ui, UiBuilder, Vec2,
    WidgetText, text::LayoutJob,
};

const DISPLAY_LIMIT: usize = 1000;
const SEARCH_MATCH_CONTEXT_LIMIT: usize = 12;

#[derive(Default)]
pub struct ClipboardButton {
    pub id: u64,
    pub texts: ClipboardButtonTexts<WidgetText>,
    pub preview: Option<(TextureHandle, Vec2)>,

    preview_background: Color32,
    with_preview_padding: Option<Vec2>,
    underline_offset: f32,
    keyboard_hint_foreground: Color32,
    search_match_background: Color32,

    label_size: f32,
    label_color: Color32,
    sublabel_size: f32,
    sublabel_color: Color32,
    muted_color: Color32,

    pin_size: f32,
    pin_color: Color32,

    color_preview: Option<Color32>,
    color_preview_size: f32,
    color_preview_corner_radius: u8,
    color_preview_background: Option<TextureHandle>,

    rendered_lengths_cache: ClipboardButtonTexts<usize>,
}

impl ClipboardButton {
    #[inline]
    pub fn id(mut self, id: u64) -> Self {
        self.id = id;
        self
    }

    #[inline]
    pub fn label(mut self, label: &str) -> Self {
        self.texts.labels =
            vec![build_layout_job(label, self.label_style(), self.muted_color).into()];
        self
    }

    #[inline]
    pub fn append_label(mut self, label: &str) -> Self {
        self.texts
            .labels
            .push(build_layout_job(label, self.label_style(), self.muted_color).into());
        self
    }

    #[inline]
    pub fn muted_label(mut self, label: &str) -> Self {
        let mut style = self.label_style();
        style.color = self.muted_color;
        self.texts.labels = vec![build_layout_job(label, style, self.muted_color).into()];
        self
    }

    #[inline]
    pub fn sublabel(mut self, sublabel: &str) -> Self {
        self.texts.sublabel =
            Some(build_layout_job(sublabel, self.sublabel_style(), self.muted_color).into());
        self
    }

    #[inline]
    pub fn preview(mut self, texture: TextureHandle, size: impl Into<Vec2>) -> Self {
        self.preview = Some((texture, size.into()));
        self
    }

    #[inline]
    pub fn preview_source(mut self, preview_source: &str) -> Self {
        self.texts.preview_source =
            Some(build_layout_job(preview_source, self.label_style(), self.muted_color).into());
        self
    }

    #[inline]
    pub fn preview_background(mut self, preview_background: impl Into<Color32>) -> Self {
        self.preview_background = preview_background.into();
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
    pub fn keyboard_hint_foreground(mut self, secondary_foreground: impl Into<Color32>) -> Self {
        self.keyboard_hint_foreground = secondary_foreground.into();
        self
    }

    #[inline]
    pub fn search_match_background(mut self, search_match_background: impl Into<Color32>) -> Self {
        self.search_match_background = search_match_background.into();
        self
    }

    #[inline]
    pub fn label_size(mut self, label_size: f32) -> Self {
        self.label_size = label_size;
        self
    }

    #[inline]
    pub fn label_color(mut self, label_color: impl Into<Color32>) -> Self {
        self.label_color = label_color.into();
        self
    }

    #[inline]
    pub fn sublabel_size(mut self, sublabel_size: f32) -> Self {
        self.sublabel_size = sublabel_size;
        self
    }

    #[inline]
    pub fn sublabel_color(mut self, sublabel_color: impl Into<Color32>) -> Self {
        self.sublabel_color = sublabel_color.into();
        self
    }

    #[inline]
    pub fn muted_color(mut self, muted_color: impl Into<Color32>) -> Self {
        self.muted_color = muted_color.into();
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

    fn label_style(&self) -> TextFormat {
        TextFormat {
            font_id: FontId::proportional(self.label_size),
            color: self.label_color,
            expand_bg: 0.0,
            ..Default::default()
        }
    }

    fn sublabel_style(&self) -> TextFormat {
        TextFormat {
            font_id: FontId::proportional(self.sublabel_size),
            color: self.sublabel_color,
            expand_bg: 0.0,
            ..Default::default()
        }
    }
}

impl ClipboardButton {
    pub fn ui(&mut self, ui: &mut Ui, state: ClipboardButtonState) -> Response {
        ui.scope_builder(
            UiBuilder::new().id_salt(self.id).sense(Sense::CLICK),
            |ui| {
                let is_active = state.is_active;
                let is_pinned = state.is_pinned;
                let keyboard_hint = state.keyboard_hint;

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

                let keyboard_hint_galley = keyboard_hint.map(|sh| {
                    WidgetText::RichText(Arc::new(
                        RichText::new(sh)
                            .size(keyboard_hint_size)
                            .color(self.keyboard_hint_foreground),
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

                let label_galleys = self
                    .texts
                    .labels
                    .iter()
                    .map(|l| {
                        l.clone().into_galley(
                            ui,
                            Some(TextWrapMode::Truncate),
                            text_width,
                            TextStyle::Button,
                        )
                    })
                    .collect::<Vec<_>>();
                let sublabel_galley = self.texts.sublabel.clone().map(|sl| {
                    sl.into_galley(
                        ui,
                        Some(TextWrapMode::Truncate),
                        text_width,
                        TextStyle::Button,
                    )
                });
                let img_src_galley = if self.preview.is_some() {
                    self.texts.preview_source.clone().map(|s| {
                        s.into_galley(
                            ui,
                            Some(TextWrapMode::Truncate),
                            text_width,
                            TextStyle::Button,
                        )
                    })
                } else {
                    None
                };

                if state.highlights.is_none() {
                    let rendered_glyph_len =
                        |g: &Galley| g.rows[0].row.glyphs.len() - if g.elided { 1 } else { 0 };

                    self.rendered_lengths_cache.labels.clear();
                    for galley in &label_galleys {
                        self.rendered_lengths_cache
                            .labels
                            .push(rendered_glyph_len(galley));
                    }
                    self.rendered_lengths_cache.sublabel =
                        sublabel_galley.as_deref().map(rendered_glyph_len);
                    self.rendered_lengths_cache.preview_source =
                        img_src_galley.as_deref().map(rendered_glyph_len);
                }

                let text_height = label_galleys.iter().fold(0.0, |acc, g| acc + g.size().y)
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

                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(desired_width, desired_height), Sense::HOVER);

                if ui.is_rect_visible(rect) {
                    let visuals = &ui.style().visuals.widgets.inactive;
                    let bg_fill = if is_active {
                        ui.style().visuals.widgets.active.weak_bg_fill
                    } else {
                        visuals.weak_bg_fill
                    };

                    let label_ellipsis = ui.painter().layout_no_wrap(
                        "…".into(),
                        FontId::proportional(self.label_size),
                        self.muted_color,
                    );
                    let sublabel_ellipsis = ui.painter().layout_no_wrap(
                        "…".into(),
                        FontId::proportional(self.sublabel_size),
                        self.muted_color,
                    );

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

                        if let Some(background_texture) = &self.color_preview_background {
                            let preview_background = Image::from_texture(background_texture)
                                .maintain_aspect_ratio(true)
                                .corner_radius(CornerRadius::same(
                                    self.color_preview_corner_radius,
                                ));
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
                    for (i, galley) in label_galleys.into_iter().enumerate() {
                        let text_pos = Pos2::new(cursor_x, cursor_y);
                        cursor_y += galley.size().y;

                        let highlight = state.highlights.and_then(|hls| hls.labels.get(i));
                        self.paint_text(
                            ui,
                            text_pos,
                            galley,
                            highlight,
                            text_width,
                            &label_ellipsis,
                        );
                    }

                    if let Some(galley) = img_src_galley {
                        let text_pos = Pos2::new(cursor_x, cursor_y);
                        let galley_height = galley.size().y;
                        let highlight =
                            state.highlights.and_then(|hls| hls.preview_source.as_ref());
                        let displayed_text_rect = self.paint_text(
                            ui,
                            text_pos,
                            galley,
                            highlight,
                            text_width,
                            &label_ellipsis,
                        );

                        // Drawing text underline manually with offset to workaround https://github.com/emilk/egui/issues/5855
                        let text_underline = Stroke {
                            width: 1.0,
                            color: visuals.text_color(),
                        };
                        let underline_y = text_pos.y + galley_height - text_underline.width
                            + self.underline_offset;
                        ui.painter().line_segment(
                            [
                                Pos2::new(text_pos.x, underline_y),
                                Pos2::new(text_pos.x + displayed_text_rect.width(), underline_y),
                            ],
                            text_underline,
                        );
                    }

                    if let Some(galley) = sublabel_galley {
                        let text_pos =
                            Pos2::new(cursor_x, rect.shrink2(padding).bottom() - galley.size().y);
                        let highlight = state.highlights.and_then(|hls| hls.sublabel.as_ref());
                        self.paint_text(
                            ui,
                            text_pos,
                            galley,
                            highlight,
                            text_width,
                            &sublabel_ellipsis,
                        );
                    }

                    if let Some(galley) = keyboard_hint_galley {
                        cursor_x += text_width + keyboard_hint_gap;
                        let text_pos =
                            Pos2::new(cursor_x, rect.center().y - galley.rect.height() / 2.0);
                        ui.painter().galley(text_pos, galley, visuals.text_color());
                    }

                    if is_pinned {
                        let pin_center = rect.min + Vec2::splat(self.pin_size / 2.0);
                        ui.painter()
                            .circle_filled(pin_center, self.pin_size, self.pin_color);
                    }
                }
            },
        )
        .response
    }

    fn paint_text(
        &self,
        ui: &Ui,
        text_pos: Pos2,
        galley: Arc<Galley>,
        highlight: Option<&ClipboardButtonHighlight>,
        text_width: f32,
        ellipsis: &Arc<Galley>,
    ) -> Rect {
        let visuals = &ui.style().visuals.widgets.inactive;
        let fallback_text_color = visuals.text_color();
        let painter = ui.painter();

        let mut galley = galley;
        let mut highlight_rects = vec![];
        let mut override_layout = None;

        if let Some(highlight) = highlight {
            if let Some(override_text) = &highlight.override_text {
                let (override_galley, prepared_override) =
                    prepare_override(ui, override_text, text_width, ellipsis.size().x);
                galley = override_galley;
                override_layout = Some(prepared_override);
            }

            highlight_rects = build_highlight_rects(&galley, &highlight.highlight);
        }

        let Some(override_layout) = override_layout else {
            for rect in highlight_rects {
                painter.rect_filled(
                    rect.translate(text_pos.to_vec2()),
                    0.0,
                    self.search_match_background,
                );
            }

            let rendered_rect = Rect::from_min_size(text_pos, galley.size());
            painter.galley(text_pos, galley, fallback_text_color);
            return rendered_rect;
        };

        let rendered_rect =
            Rect::from_min_size(text_pos, egui::vec2(override_layout.width, galley.size().y));
        let mut text_pos = text_pos;
        if override_layout.leading_ellipsis {
            painter.galley(text_pos, Arc::clone(ellipsis), fallback_text_color);
            text_pos.x += ellipsis.size().x;
        }

        let clip_rect = Rect::from_min_size(
            text_pos + egui::vec2(override_layout.clip_pad_left, 0.0),
            egui::vec2(
                override_layout.clip_width
                    - override_layout.clip_pad_left
                    - override_layout.clip_pad_right,
                galley.size().y,
            ),
        );
        text_pos.x -= galley.rows[0].row.glyphs[override_layout.start_glyph_idx]
            .pos
            .x;

        let clipped = painter.with_clip_rect(clip_rect);
        for rect in highlight_rects {
            clipped.rect_filled(
                rect.translate(text_pos.to_vec2()),
                0.0,
                self.search_match_background,
            );
        }
        clipped.galley(text_pos, galley, fallback_text_color);

        if override_layout.trailing_ellipsis {
            painter.galley(
                egui::pos2(clip_rect.max.x + override_layout.clip_pad_right, text_pos.y),
                Arc::clone(ellipsis),
                fallback_text_color,
            );
        }

        rendered_rect
    }

    pub fn build_highlight(
        &self,
        texts_with_highlights: ClipboardButtonTexts<(&str, &[u32])>,
    ) -> ClipboardButtonTexts<ClipboardButtonHighlight> {
        let rendered_lengths = &self.rendered_lengths_cache;
        ClipboardButtonTexts {
            labels: texts_with_highlights
                .labels
                .iter()
                .zip(&rendered_lengths.labels)
                .map(|(&(text, matched), &rendered_len)| {
                    self.build_text_highlight(text, matched, rendered_len, self.label_style())
                })
                .collect(),
            sublabel: texts_with_highlights
                .sublabel
                .zip(rendered_lengths.sublabel)
                .map(|((text, matched), rendered_len)| {
                    self.build_text_highlight(text, matched, rendered_len, self.sublabel_style())
                }),
            preview_source: texts_with_highlights
                .preview_source
                .zip(rendered_lengths.preview_source)
                .map(|((text, matched), rendered_len)| {
                    self.build_text_highlight(text, matched, rendered_len, self.label_style())
                }),
        }
    }

    fn build_text_highlight(
        &self,
        text: &str,
        matched: &[u32],
        rendered_len: usize,
        style: TextFormat,
    ) -> ClipboardButtonHighlight {
        if matched.is_empty() {
            return ClipboardButtonHighlight::default();
        }

        let (leading_ws_byte_end, trailing_ws_byte_start) = text_whitespace_bounds(text);
        let mut hl_tracker = HighlightTracker::new(matched, 0);
        let mut needs_override_text = true;
        let mut text_processed_fully = true;
        for (i, (display, c, byte_idx, _)) in
            iter_char_to_display_text(text, leading_ws_byte_end, trailing_ws_byte_start).enumerate()
        {
            let was_highlighted = hl_tracker.start_char_idx.is_some();
            let highlighted = hl_tracker.track(c, byte_idx);
            if was_highlighted && !highlighted {
                // A highlighted group can be rendered fully
                needs_override_text = false;
            }

            if i > DISPLAY_LIMIT || hl_tracker.display_len == rendered_len {
                text_processed_fully = false;
                break;
            }
            hl_tracker.advance(display);
        }

        if text_processed_fully {
            needs_override_text = false;
        }

        if needs_override_text {
            let (highlight, override_text) = build_override_text_with_highlight(
                text,
                matched,
                style,
                self.muted_color,
                leading_ws_byte_end,
                trailing_ws_byte_start,
            );
            ClipboardButtonHighlight {
                highlight,
                override_text: Some(override_text),
            }
        } else {
            ClipboardButtonHighlight {
                highlight: hl_tracker.finish(),
                override_text: None,
            }
        }
    }
}

fn build_layout_job(text: &str, default_text_format: TextFormat, muted_fg: Color32) -> LayoutJob {
    let mut job_builder = LayoutJobBuilder::new(default_text_format, muted_fg);
    if text.is_empty() {
        return job_builder.finish();
    }

    let (leading_ws_byte_end, trailing_ws_byte_start) = text_whitespace_bounds(text);
    for (char_idx, (display, _, _, muted)) in
        iter_char_to_display_text(text, leading_ws_byte_end, trailing_ws_byte_start).enumerate()
    {
        // Very very long string causes egui to choke on first render,
        // even when we only display it on a single line with small width
        if char_idx > DISPLAY_LIMIT {
            // TODO: use the has_trailing mechanic just like OverrideText instead of this (aka doing
            // this in ui() for consistency)
            job_builder.push("…", true);
            return job_builder.finish();
        }

        job_builder.push(display, muted);
    }

    job_builder.finish()
}

fn prepare_override(
    ui: &Ui,
    override_text: &OverrideText,
    text_width: f32,
    ellipsis_width: f32,
) -> (Arc<Galley>, PreparedOverride) {
    let galley = override_text.text.clone().into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Button,
    );

    let row = &galley.rows[0].row;
    let mut start_glyph_idx = override_text.start_glyph_idx;
    let mut render_to_end_width = row.size.x - row.glyphs[start_glyph_idx].pos.x;

    let leading_ellipsis = override_text.has_leading || override_text.start_glyph_idx > 0;
    let leading_ellipsis_width = if leading_ellipsis {
        ellipsis_width
    } else {
        0.0
    };
    let trailing_ellipsis =
        override_text.has_trailing || render_to_end_width > text_width - leading_ellipsis_width;
    let trailing_ellipsis_width = if trailing_ellipsis {
        ellipsis_width
    } else {
        0.0
    };

    // If the highlight-focused galley has empty space at the end,
    // scroll glyphs before the start glyph in to fill it
    let renderable_width = text_width - leading_ellipsis_width - trailing_ellipsis_width;
    loop {
        if start_glyph_idx == 0 {
            break;
        }

        render_to_end_width +=
            row.glyphs[start_glyph_idx].pos.x - row.glyphs[start_glyph_idx - 1].pos.x;
        if render_to_end_width > renderable_width {
            break;
        }

        start_glyph_idx -= 1;
    }

    // Hide overhangs of the last start unrendered glyph and the first end unrendered glyph
    let clip_pad_left = if start_glyph_idx > 0 {
        let start_glyph = row.glyphs[start_glyph_idx];
        let last_start_unrendered_glyph = row.glyphs[start_glyph_idx - 1];

        let glyph_width = start_glyph.pos.x - last_start_unrendered_glyph.pos.x;
        let uv_width = last_start_unrendered_glyph.uv_rect.offset.x
            + last_start_unrendered_glyph.uv_rect.size.x;
        (uv_width - glyph_width).max(0.0)
    } else {
        0.0
    };
    let mut clip_pad_right = 0.0;

    let (clip_width, width) = if trailing_ellipsis {
        // Snap the clip width to the last fully visible glyph
        let mut total_render_glyph_width = 0.0;
        let mut prev_glyph_idx = start_glyph_idx;
        for i in start_glyph_idx + 1..row.glyphs.len() + 1 {
            let glyph_x_max = if i < row.glyphs.len() {
                row.glyphs[i].pos.x
            } else {
                row.size.x
            };
            let new_total_width =
                total_render_glyph_width + glyph_x_max - row.glyphs[prev_glyph_idx].pos.x;
            if new_total_width > renderable_width {
                break;
            }

            total_render_glyph_width = new_total_width;
            prev_glyph_idx = i;
        }

        let end_glyph_idx = prev_glyph_idx - 1;
        if end_glyph_idx > 0 && end_glyph_idx < row.glyphs.len() - 1 {
            let first_end_unrendered_glyph = row.glyphs[end_glyph_idx + 1];
            let glyph_x_max = if end_glyph_idx < row.glyphs.len() - 2 {
                row.glyphs[end_glyph_idx + 2].pos.x
            } else {
                row.size.x
            };

            let glyph_width = glyph_x_max - first_end_unrendered_glyph.pos.x;
            let uv_width = first_end_unrendered_glyph.uv_rect.offset.x
                + first_end_unrendered_glyph.uv_rect.size.x;
            clip_pad_right = (uv_width - glyph_width).max(0.0);
        }

        (
            total_render_glyph_width,
            leading_ellipsis_width + total_render_glyph_width + trailing_ellipsis_width,
        )
    } else {
        let content_width = row.size.x - row.glyphs[start_glyph_idx].pos.x;
        (renderable_width, leading_ellipsis_width + content_width)
    };

    (
        galley,
        PreparedOverride {
            start_glyph_idx,
            leading_ellipsis,
            trailing_ellipsis,
            clip_width,
            clip_pad_left,
            clip_pad_right,
            width,
        },
    )
}

fn build_highlight_rects(galley: &Galley, highlight: &[Range<usize>]) -> Vec<Rect> {
    // LayoutJob's TextFormat's `background` only highlights each section separatedly,
    // and each section can have different height and position
    let placed_row = &galley.rows[0];
    let row = &placed_row.row;
    let row_top = placed_row.pos.y;
    let row_bottom = row_top + row.size.y;

    let (elide_len, elide_pos) = if galley.elided && galley.job.wrap.overflow_character.is_some() {
        (1, row.glyphs.last().map(|g| g.pos.x))
    } else {
        (0, None)
    };
    let glyph_len = row.glyphs.len() - elide_len;

    let mut rects = Vec::with_capacity(highlight.len());
    for &Range { start, end } in highlight {
        if start >= glyph_len {
            break;
        }
        let end = end.min(glyph_len);

        let x_min = placed_row.pos.x + row.glyphs[start].pos.x;
        let x_max = if end < glyph_len {
            placed_row.pos.x + row.glyphs[end].pos.x
        } else {
            placed_row.pos.x + elide_pos.unwrap_or(row.size.x)
        };

        rects.push(Rect::from_min_max(
            egui::pos2(x_min, row_top),
            egui::pos2(x_max, row_bottom),
        ));
    }

    rects
}

fn build_override_text_with_highlight(
    text: &str,
    r#match: &[u32],
    default_text_format: TextFormat,
    muted_fg: Color32,
    leading_ws_byte_end: usize,
    trailing_ws_byte_start: usize,
) -> (Vec<Range<usize>>, OverrideText) {
    let (substr, start, end, display_start) = get_override_substr(text, r#match[0] as usize);
    let has_leading = start > 0;
    let has_trailing = end < text.len();

    let leading_ws_byte_end = leading_ws_byte_end.saturating_sub(start).min(end);
    let trailing_ws_byte_start = trailing_ws_byte_start.saturating_sub(start).min(end);

    let mut job_builder = LayoutJobBuilder::new(default_text_format, muted_fg);
    let mut hl_tracker = HighlightTracker::new(r#match, start);
    for (display, c, byte_idx, muted) in
        iter_char_to_display_text(substr, leading_ws_byte_end, trailing_ws_byte_start)
    {
        hl_tracker.track(c, byte_idx);
        job_builder.push(display, muted);
        hl_tracker.advance(display);
    }

    (
        hl_tracker.finish(),
        OverrideText {
            text: job_builder.finish().into(),
            start_glyph_idx: display_start,
            has_leading,
            has_trailing,
        },
    )
}

fn get_override_substr(s: &str, idx: usize) -> (&str, usize, usize, usize) {
    let mut start = idx;
    while !s.is_char_boundary(start) {
        start -= 1;
    }

    for _ in 0..SEARCH_MATCH_CONTEXT_LIMIT {
        if let Some(cb) = prev_char_boundary(s, start) {
            start = cb;
        } else {
            break;
        };
    }

    let tail = &s[start..];

    let mut it = tail.char_indices();
    let mut count = 0;
    let mut end = tail.len();
    for (i, _) in it.by_ref() {
        if count == DISPLAY_LIMIT {
            end = i;
            break;
        }
        count += 1;
    }

    if count < DISPLAY_LIMIT {
        let mut display_start = 0;
        let end = start + end;
        while count < DISPLAY_LIMIT {
            if let Some(cb) = prev_char_boundary(s, start) {
                start = cb;
            } else {
                break;
            };

            count += 1;
            display_start += 1;
        }

        (&s[start..end], start, end, display_start)
    } else {
        (&tail[..end], start, start + end, 0)
    }
}

fn iter_char_to_display_text(
    text: &str,
    leading_ws_byte_end: usize,
    trailing_ws_byte_start: usize,
) -> impl Iterator<Item = (&str, char, usize, bool)> {
    text.chars().scan(
        (leading_ws_byte_end, trailing_ws_byte_start, 0),
        |(leading_ws_byte_end, trailing_ws_byte_start, byte_idx), c| {
            let bi = *byte_idx;
            *byte_idx += c.len_utf8();

            let is_leading_ws = bi < *leading_ws_byte_end;
            let is_trailing_ws = bi >= *trailing_ws_byte_start;

            let (display_text, is_display_ws) =
                to_display_whitespace(c, is_leading_ws || is_trailing_ws)
                    .map(|s| (s, true))
                    .unwrap_or((&text[bi..bi + c.len_utf8()], false));

            Some((display_text, c, bi, is_display_ws))
        },
    )
}

fn to_display_whitespace(c: char, convert_space: bool) -> Option<&'static str> {
    Some(match c {
        ' ' if convert_space => "·",
        '\t' => " ⇥ ",
        '\n' => "↵",
        '\r' => "␍",
        '\x0C' => "␌",
        '\x0B' => "␋",
        _ => return None,
    })
}

fn text_whitespace_bounds(text: &str) -> (usize, usize) {
    let leading_ws_byte_end = text.chars().take_while(|c| c.is_ascii_whitespace()).count();
    let trailing_ws_byte_start = text.len()
        - text
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_whitespace())
            .count();
    (leading_ws_byte_end, trailing_ws_byte_start)
}

fn prev_char_boundary(s: &str, mut idx: usize) -> Option<usize> {
    loop {
        if idx == 0 {
            return None;
        }

        idx -= 1;
        if s.is_char_boundary(idx) {
            return Some(idx);
        }
    }
}

struct HighlightTracker<'a> {
    matched: Peekable<Iter<'a, u32>>,
    byte_origin: usize,
    highlights: Vec<Range<usize>>,
    start_char_idx: Option<usize>,
    display_len: usize,
}

impl<'a> HighlightTracker<'a> {
    fn new(matched: &'a [u32], byte_origin: usize) -> Self {
        Self {
            matched: matched.iter().peekable(),
            byte_origin,
            highlights: vec![],
            start_char_idx: None,
            display_len: 0,
        }
    }

    fn track(&mut self, c: char, byte_idx: usize) -> bool {
        let start = byte_idx + self.byte_origin;
        let end = start + c.len_utf8();
        let mut highlighted = false;
        while let Some(&&matched_byte) = self.matched.peek()
            && (start..end).contains(&(matched_byte as usize))
        {
            highlighted = true;
            self.matched.next();
        }

        if highlighted != self.start_char_idx.is_some() {
            if highlighted {
                self.start_char_idx = Some(self.display_len);
            } else {
                self.highlights
                    .push((self.start_char_idx.take().unwrap()..self.display_len).into());
            }
        }

        highlighted
    }

    fn advance(&mut self, display: &str) {
        self.display_len += display.chars().count();
    }

    fn finish(mut self) -> Vec<Range<usize>> {
        if let Some(start_idx) = self.start_char_idx.take() {
            self.highlights.push((start_idx..self.display_len).into());
        }
        self.highlights
    }
}

struct LayoutJobBuilder {
    job: LayoutJob,
    run: String,
    prev_muted: bool,
    default_text_format: TextFormat,
    muted_fg: Color32,
}

impl LayoutJobBuilder {
    fn new(default_text_format: TextFormat, muted_fg: Color32) -> Self {
        Self {
            job: LayoutJob::default(),
            run: String::new(),
            prev_muted: false,
            default_text_format,
            muted_fg,
        }
    }

    fn push(&mut self, display: &str, muted: bool) {
        if muted != self.prev_muted && !self.run.is_empty() {
            self.flush_run();
        }
        self.run.push_str(display);
        self.prev_muted = muted;
    }

    fn finish(mut self) -> LayoutJob {
        if !self.run.is_empty() {
            self.flush_run();
        }
        self.job
    }

    fn flush_run(&mut self) {
        let mut format = self.default_text_format.clone();
        if self.prev_muted {
            format.color = self.muted_fg;
        }
        self.job.append(&mem::take(&mut self.run), 0.0, format);
    }
}

struct PreparedOverride {
    start_glyph_idx: usize,
    leading_ellipsis: bool,
    trailing_ellipsis: bool,
    clip_width: f32,
    clip_pad_left: f32,
    clip_pad_right: f32,
    width: f32,
}

#[derive(Default)]
pub struct ClipboardButtonState<'a> {
    is_active: bool,
    is_pinned: bool,
    keyboard_hint: Option<&'static str>,
    highlights: Option<&'a ClipboardButtonTexts<ClipboardButtonHighlight>>,
}

impl<'a> ClipboardButtonState<'a> {
    #[inline]
    pub fn is_active(mut self, is_active: bool) -> Self {
        self.is_active = is_active;
        self
    }

    #[inline]
    pub fn is_pinned(mut self, is_pinned: bool) -> Self {
        self.is_pinned = is_pinned;
        self
    }

    #[inline]
    pub fn keyboard_hint(mut self, keyboard_hint: &'static str) -> Self {
        self.keyboard_hint = Some(keyboard_hint);
        self
    }

    #[inline]
    pub fn highlights(
        mut self,
        highlights: &'a ClipboardButtonTexts<ClipboardButtonHighlight>,
    ) -> Self {
        self.highlights = Some(highlights);
        self
    }
}

#[derive(Debug, Default)]
pub struct ClipboardButtonHighlight {
    highlight: Vec<Range<usize>>,
    override_text: Option<OverrideText>,
}

#[derive(Debug)]
struct OverrideText {
    text: WidgetText,
    start_glyph_idx: usize,
    has_leading: bool,
    has_trailing: bool,
}

#[derive(Debug)]
pub struct ClipboardButtonTexts<T> {
    pub labels: Vec<T>,
    pub sublabel: Option<T>,
    pub preview_source: Option<T>,
}

impl<T> Default for ClipboardButtonTexts<T> {
    fn default() -> Self {
        Self {
            labels: Default::default(),
            sublabel: Default::default(),
            preview_source: Default::default(),
        }
    }
}
