use std::{mem, range::Range, sync::Arc};

use egui::{
    Color32, CornerRadius, FontId, Image, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, TextFormat, TextStyle, TextWrapMode, TextureHandle, Ui, UiBuilder, Vec2,
    WidgetText, text::LayoutJob,
};

const DISPLAY_LIMIT: usize = 1_000;
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
                    self.rendered_lengths_cache.labels.clear();
                    for galley in &label_galleys {
                        self.rendered_lengths_cache.labels.push(
                            galley.rows[0].row.glyphs.len() - if galley.elided { 1 } else { 0 },
                        );
                    }
                    self.rendered_lengths_cache.sublabel = sublabel_galley.as_ref().map(|galley| {
                        galley.rows[0].row.glyphs.len() - if galley.elided { 1 } else { 0 }
                    });
                    self.rendered_lengths_cache.preview_source =
                        img_src_galley.as_ref().map(|galley| {
                            galley.rows[0].row.glyphs.len() - if galley.elided { 1 } else { 0 }
                        });
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

                // TODO: can we use this to prevent cloning widget texts?
                if ui.is_rect_visible(rect) {
                    let visuals = &ui.style().visuals.widgets.inactive;
                    let bg_fill = if is_active {
                        ui.style().visuals.widgets.active.weak_bg_fill
                    } else {
                        visuals.weak_bg_fill
                    };

                    let label_ell = ui.painter().layout_no_wrap(
                        "…".into(),
                        FontId::proportional(self.label_size),
                        self.muted_color,
                    );
                    let label_ell_w = label_ell.size().x;
                    let sublabel_ell = ui.painter().layout_no_wrap(
                        "…".into(),
                        FontId::proportional(self.sublabel_size),
                        self.muted_color,
                    );
                    let sublabel_ell_w = sublabel_ell.size().x;

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
                    for mut galley in label_galleys {
                        let mut text_pos = Pos2::new(cursor_x, cursor_y);
                        cursor_y += galley.size().y;

                        // TODO: do this for other texts than just the first label
                        let mut override_stuffs = None;
                        let mut highlight_rects = vec![];
                        if let Some(hl) = state.highlights
                            && !hl.labels.is_empty()
                        {
                            if let Some(override_text) = &hl.labels[0].override_text {
                                let override_galley = override_text.text.clone().into_galley(
                                    ui,
                                    Some(TextWrapMode::Extend),
                                    f32::INFINITY,
                                    TextStyle::Button,
                                );

                                let placed_row = &override_galley.rows[0];
                                let row = &placed_row.row;
                                let mut start_glyph_idx = override_text.start_glyph_idx;
                                let start_glyph = row.glyphs[start_glyph_idx];
                                let mut render_to_end_width = row.size.x - start_glyph.pos.x;

                                let leading_ellipsis =
                                    override_text.has_leading || override_text.start_glyph_idx > 0;
                                let trailing_ellipsis = override_text.has_trailing
                                    || render_to_end_width
                                        > text_width
                                            - if leading_ellipsis { label_ell_w } else { 0.0 };

                                // If the highlight-focused galley has empty space at the end,
                                // scroll some of the start glyphs in to fill it
                                let render_width = text_width
                                    - if leading_ellipsis { label_ell_w } else { 0.0 }
                                    - if trailing_ellipsis { label_ell_w } else { 0.0 };
                                loop {
                                    if start_glyph_idx == 0 {
                                        break;
                                    }

                                    render_to_end_width += row.glyphs[start_glyph_idx].pos.x
                                        - row.glyphs[start_glyph_idx - 1].pos.x;
                                    if render_to_end_width > render_width {
                                        break;
                                    }

                                    start_glyph_idx -= 1;
                                }

                                override_stuffs =
                                    Some((start_glyph_idx, leading_ellipsis, trailing_ellipsis));
                                galley = override_galley;
                            }

                            let placed_row = &galley.rows[0];
                            let row = &placed_row.row;
                            let row_top = placed_row.pos.y;
                            let row_bottom = row_top + row.size.y;

                            let (elide_len, elide_pos) =
                                if galley.elided && galley.job.wrap.overflow_character.is_some() {
                                    (1, row.glyphs.last().map(|g| g.pos.x))
                                } else {
                                    (0, None)
                                };

                            // LayoutJob's TextFormat's `background` only highlights each section separatedly,
                            // and each section can have different height and position
                            for &Range { start, end } in &hl.labels[0].highlight {
                                let glyph_len = row.glyphs.len() - elide_len;
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

                                highlight_rects.push(Rect::from_min_max(
                                    egui::pos2(x_min, row_top),
                                    egui::pos2(x_max, row_bottom),
                                ));
                            }
                        }

                        if let Some((start_glyph_idx, leading_ellipsis, trailing_ellipsis)) =
                            override_stuffs
                        {
                            let placed_row = &galley.rows[0];
                            let row = &placed_row.row;
                            let start_glyph = row.glyphs[start_glyph_idx];

                            let mut clip_width = text_width;
                            if leading_ellipsis {
                                ui.painter().galley(
                                    text_pos,
                                    label_ell.clone(),
                                    visuals.text_color(),
                                );

                                text_pos += egui::vec2(label_ell_w, 0.0);
                                clip_width -= label_ell_w;
                            }
                            if trailing_ellipsis {
                                clip_width -= label_ell_w;

                                let mut total_render_glyph_width = 0.0;
                                let mut prev_glyph_x = start_glyph.pos.x;
                                for i in start_glyph_idx + 1..row.glyphs.len() {
                                    let new_total_width = total_render_glyph_width
                                        + row.glyphs[i].pos.x
                                        - prev_glyph_x;
                                    if new_total_width > clip_width {
                                        break;
                                    }

                                    total_render_glyph_width = new_total_width;
                                    prev_glyph_x = row.glyphs[i].pos.x;
                                }
                                clip_width = total_render_glyph_width;
                            }

                            let clip_rect = Rect::from_min_size(
                                text_pos,
                                egui::vec2(clip_width, galley.size().y),
                            );

                            text_pos -= egui::vec2(start_glyph.pos.x, 0.0);

                            let p = ui.painter().with_clip_rect(clip_rect);
                            for hlr in highlight_rects {
                                p.rect_filled(
                                    hlr.translate(text_pos.to_vec2()),
                                    0.0,
                                    self.search_match_background,
                                );
                            }
                            p.galley(text_pos, galley, visuals.text_color());

                            if trailing_ellipsis {
                                ui.painter().galley(
                                    egui::pos2(clip_rect.max.x, text_pos.y),
                                    label_ell.clone(),
                                    visuals.text_color(),
                                );
                            }
                        } else {
                            for hlr in highlight_rects {
                                ui.painter().rect_filled(
                                    hlr.translate(text_pos.to_vec2()),
                                    0.0,
                                    self.search_match_background,
                                );
                            }
                            ui.painter().galley(text_pos, galley, visuals.text_color());
                        }
                    }

                    if let Some(galley) = img_src_galley {
                        let text_pos = Pos2::new(cursor_x, cursor_y);
                        let text_underline = Stroke {
                            width: 1.0,
                            color: visuals.text_color(),
                        };

                        // Drawing text underline manually with offset to workaround https://github.com/emilk/egui/issues/5855
                        let underline_y = text_pos.y + galley.size().y - text_underline.width
                            + self.underline_offset;
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
                        ui.painter().galley(text_pos, galley, visuals.text_color());
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

    // TODO: build other texts than just the first label
    pub fn build_highlight(
        &self,
        texts_with_highlights: ClipboardButtonTexts<(&str, &[u32])>,
    ) -> ClipboardButtonTexts<ClipboardButtonHighlight> {
        if texts_with_highlights.labels.is_empty() || self.rendered_lengths_cache.labels.is_empty()
        {
            return ClipboardButtonTexts {
                labels: vec![],
                sublabel: None,
                preview_source: None,
            };
        }

        let (plain, m) = texts_with_highlights.labels[0];
        let rendered_len = self.rendered_lengths_cache.labels[0];
        let default_text_format = self.label_style();

        if m.is_empty() {
            return ClipboardButtonTexts {
                labels: vec![],
                sublabel: None,
                preview_source: None,
            };
        }

        let leading_ws_byte_end = plain
            .chars()
            .take_while(|c| c.is_ascii_whitespace())
            .count();
        let trailing_ws_byte_start = plain.len()
            - plain
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_whitespace())
                .count();

        let mut match_iter = m.iter().peekable();
        let mut highlight: Vec<Range<usize>> = vec![];
        let mut override_text = None;

        let mut needs_override_text = true;
        let mut last_display_char_idx = 0;
        let mut display_char_idx = 0;
        let mut highlight_start_char_idx = None;
        for (i, (s, c, bi, _)) in
            iter_char_to_display_text(plain, leading_ws_byte_end, trailing_ws_byte_start)
                .enumerate()
        {
            let mut highlighted = false;
            while let Some(&&matched_byte) = match_iter.peek()
                && (bi..bi + c.len_utf8()).contains(&(matched_byte as usize))
            {
                highlighted = true;
                match_iter.next();
            }

            let prev_highlighted = highlight_start_char_idx.is_some();
            if highlighted != prev_highlighted {
                if highlighted {
                    highlight_start_char_idx = Some(display_char_idx);
                } else {
                    highlight.push((highlight_start_char_idx.unwrap()..display_char_idx).into());
                    highlight_start_char_idx = None;
                }
            }

            // A highlighted group can be rendered fully
            if prev_highlighted && !highlighted {
                needs_override_text = false;
            }

            if i > DISPLAY_LIMIT {
                break;
            }

            if display_char_idx == rendered_len {
                break;
            }

            display_char_idx += s.chars().count();
            last_display_char_idx = display_char_idx;
        }

        if let Some(start_idx) = highlight_start_char_idx {
            highlight.push((start_idx..last_display_char_idx + 1).into());
        }

        if needs_override_text {
            let (hl, ot) = build_override_text_with_highlight(
                plain,
                m,
                default_text_format,
                self.muted_color,
                leading_ws_byte_end,
                trailing_ws_byte_start,
            );
            highlight = hl;
            override_text = Some(ot);
        }

        ClipboardButtonTexts {
            labels: vec![ClipboardButtonHighlight {
                highlight,
                override_text,
            }],
            sublabel: None,
            preview_source: None,
        }
    }
}

fn build_override_text_with_highlight(
    text: &str,
    r#match: &[u32],
    default_text_format: TextFormat,
    muted_fg: Color32,
    leading_ws_byte_end: usize,
    trailing_ws_byte_start: usize,
) -> (Vec<Range<usize>>, OverrideText) {
    let mut job = LayoutJob::default();
    let mut highlights = vec![];

    if text.is_empty() {
        return (
            highlights,
            OverrideText {
                text: job.into(),
                start_glyph_idx: 0,
                has_leading: false,
                has_trailing: false,
            },
        );
    }

    let first_match_byte_idx = r#match[0];
    let (substr, start, end, display_start) =
        get_override_substr(text, first_match_byte_idx as usize);
    let has_leading = start > 0;
    let has_trailing = end < text.len();

    let leading_ws_byte_end = leading_ws_byte_end.saturating_sub(start).min(end);
    let trailing_ws_byte_start = trailing_ws_byte_start.saturating_sub(start).min(end);

    let mut match_iter = r#match.iter().peekable();
    let mut part_text = String::new();
    let mut last_display_char_idx = 0;
    let mut display_char_idx = 0;
    let mut prev_muted = false;
    let mut highlight_start_char_idx = None;
    for (ds, c, bi, is_display_ws) in
        iter_char_to_display_text(substr, leading_ws_byte_end, trailing_ws_byte_start)
    {
        let mut highlighted = false;
        while let Some(&&matched_byte) = match_iter.peek()
            && (bi..bi + c.len_utf8()).contains(&(matched_byte as usize))
        {
            highlighted = true;
            match_iter.next();
        }

        let prev_highlighted = highlight_start_char_idx.is_some();
        if highlighted != prev_highlighted {
            if highlighted {
                highlight_start_char_idx = Some(display_char_idx);
            } else {
                highlights.push((highlight_start_char_idx.unwrap()..display_char_idx).into());
                highlight_start_char_idx = None;
            }
        }

        let muted = is_display_ws;
        if muted != prev_muted && !part_text.is_empty() {
            let mut tf = default_text_format.clone();
            if prev_muted {
                tf.color = muted_fg;
            }
            job.append(&mem::take(&mut part_text), 0.0, tf);
        }

        part_text.push_str(ds);

        display_char_idx += ds.chars().count();
        last_display_char_idx = display_char_idx;
        prev_muted = muted;
    }

    if !part_text.is_empty() {
        let mut tf = default_text_format.clone();
        if prev_muted {
            tf.color = muted_fg;
        }
        job.append(&mem::take(&mut part_text), 0.0, tf);
    }

    if let Some(start_idx) = highlight_start_char_idx {
        highlights.push((start_idx..last_display_char_idx + 1).into());
    }

    (
        highlights,
        OverrideText {
            text: job.into(),
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
        if start == 0 {
            break;
        }

        start -= 1;
        while !s.is_char_boundary(start) {
            start -= 1;
        }
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
            if start == 0 {
                break;
            }

            start -= 1;
            while !s.is_char_boundary(start) {
                start -= 1;
            }
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

fn build_layout_job(text: &str, default_text_format: TextFormat, muted_fg: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    if text.is_empty() {
        return job;
    }

    let trailing_ws_byte_start = text.len()
        - text
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_whitespace())
            .count();

    let mut part_text = String::new();
    let mut byte_idx = 0;
    let mut prev_muted = false;
    let mut is_leading_ws = true;
    for (char_idx, c) in text.chars().enumerate() {
        // Very very long string causes egui to choke on first render,
        // even when we only display it on a single line with small width
        if char_idx > DISPLAY_LIMIT {
            if !part_text.is_empty() {
                let mut tf = default_text_format.clone();
                if prev_muted {
                    tf.color = muted_fg;
                }
                job.append(&mem::take(&mut part_text), 0.0, tf);
            }

            // TODO: use the has_trailing mechanic just like OverrideText instead of this (aka doing
            // this in ui() for consistency)
            let mut tf = default_text_format.clone();
            tf.color = muted_fg;
            job.append("…", 0.0, tf);

            return job;
        }

        if is_leading_ws && !c.is_ascii_whitespace() {
            is_leading_ws = false;
        }

        let char_byte_len = c.len_utf8();
        let is_trailing_ws = byte_idx >= trailing_ws_byte_start;
        let display_ws = to_display_whitespace(c, is_leading_ws || is_trailing_ws);
        let muted = display_ws.is_some();

        if muted != prev_muted && !part_text.is_empty() {
            let mut tf = default_text_format.clone();
            if prev_muted {
                tf.color = muted_fg;
            }
            job.append(&mem::take(&mut part_text), 0.0, tf);
        }

        if let Some(ws) = display_ws {
            part_text.push_str(ws);
        } else {
            part_text.push(c);
        }

        prev_muted = muted;
        byte_idx += char_byte_len;
    }

    if !part_text.is_empty() {
        let mut tf = default_text_format.clone();
        if prev_muted {
            tf.color = muted_fg;
        }
        job.append(&mem::take(&mut part_text), 0.0, tf);
    }

    job
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

pub struct ClipboardButtonHighlight {
    highlight: Vec<Range<usize>>,
    override_text: Option<OverrideText>,
}

struct OverrideText {
    text: WidgetText,
    start_glyph_idx: usize,
    has_leading: bool,
    has_trailing: bool,
}

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
