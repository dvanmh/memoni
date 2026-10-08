use std::{iter::Peekable, mem, range::Range, slice::Iter, sync::Arc};

use egui::{
    Color32, CornerRadius, FontId, Galley, Image, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, TextFormat, TextStyle, TextWrapMode, TextureHandle, Ui, UiBuilder, Vec2,
    WidgetText,
    text::{ByteIndex, LayoutJob, LayoutSection},
};

// Very very long string causes egui to choke on first render,
// even when we only display it on a single line with small width.
const DISPLAY_LIMIT: usize = 1000;

const SEARCH_MATCH_CONTEXT_LIMIT: usize = 12;

const ELLIPSIS: &str = "…";

#[derive(Default)]
pub struct ClipboardButton {
    pub id: u64,
    pub preview: Option<(TextureHandle, Vec2)>,
    texts: ButtonTexts<DisplayText>,

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

    text_cache: ButtonTexts<PaintText>,
}

impl ClipboardButton {
    #[inline]
    pub fn id(mut self, id: u64) -> Self {
        self.id = id;
        self
    }

    #[inline]
    pub fn label(mut self, label: &str) -> Self {
        self.texts.labels = vec![build_display_text(
            label,
            self.label_style(),
            self.muted_color,
        )];
        self
    }

    #[inline]
    pub fn append_label(mut self, label: &str) -> Self {
        self.texts.labels.push(build_display_text(
            label,
            self.label_style(),
            self.muted_color,
        ));
        self
    }

    #[inline]
    pub fn muted_label(mut self, label: &str) -> Self {
        let style = self.text_style(self.label_size, self.muted_color);
        self.texts.labels = vec![build_display_text(label, style, self.muted_color)];
        self
    }

    #[inline]
    pub fn sublabel(mut self, sublabel: &str) -> Self {
        self.texts.sublabel = Some(build_display_text(
            sublabel,
            self.sublabel_style(),
            self.muted_color,
        ));
        self
    }

    #[inline]
    pub fn preview(mut self, texture: TextureHandle, size: impl Into<Vec2>) -> Self {
        self.preview = Some((texture, size.into()));
        self
    }

    #[inline]
    pub fn preview_source(mut self, preview_source: &str) -> Self {
        self.texts.preview_source = Some(build_display_text(
            preview_source,
            self.label_style(),
            self.muted_color,
        ));
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

    fn text_style(&self, size: f32, color: Color32) -> TextFormat {
        TextFormat {
            font_id: FontId::proportional(size),
            color,
            expand_bg: 0.0,
            ..Default::default()
        }
    }

    fn label_style(&self) -> TextFormat {
        self.text_style(self.label_size, self.label_color)
    }

    fn sublabel_style(&self) -> TextFormat {
        self.text_style(self.sublabel_size, self.sublabel_color)
    }

    pub fn ui(&mut self, ui: &mut Ui, state: ClipboardButtonState) -> Response {
        ui.scope_builder(
            UiBuilder::new().id_salt(self.id).sense(Sense::CLICK),
            |ui| {
                let is_active = state.is_active;
                let is_pinned = state.is_pinned;
                let keyboard_hint = state.keyboard_hint;
                let matches = state.matches.as_ref();
                let raw = state.raw.as_ref();

                // TODO: make these configurable?
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

                let label_height = display_text_height(ui, self.label_style());
                let sublabel_height = if raw.is_some() {
                    Some(display_text_height(ui, self.sublabel_style()))
                } else {
                    self.texts
                        .sublabel
                        .as_ref()
                        .map(|_| display_text_height(ui, self.sublabel_style()))
                };
                let img_src_height = if self.preview.is_some() && raw.is_none() {
                    self.texts
                        .preview_source
                        .as_ref()
                        .map(|_| display_text_height(ui, self.label_style()))
                } else {
                    None
                };

                let label_count = if raw.is_some() {
                    1
                } else {
                    self.texts.labels.len()
                };
                let text_height = (label_count as f32) * label_height
                    + sublabel_height.unwrap_or(0.0)
                    + img_src_height.unwrap_or(0.0);
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

                    let label_ellipsis = Arc::new(ui.painter().layout_no_wrap(
                        ELLIPSIS.into(),
                        FontId::proportional(self.label_size),
                        self.muted_color,
                    ));
                    let sublabel_ellipsis = Arc::new(ui.painter().layout_no_wrap(
                        ELLIPSIS.into(),
                        FontId::proportional(self.sublabel_size),
                        self.muted_color,
                    ));
                    let label_style = self.label_style();
                    let sublabel_style = self.sublabel_style();

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
                    if let Some(raw) = raw {
                        let text_pos = Pos2::new(cursor_x, cursor_y);
                        cursor_y += label_height;

                        let text = self.text_cache.label(0, || {
                            let display_text =
                                build_display_text(raw.text, label_style.clone(), self.muted_color);
                            build_paint_text(
                                ui,
                                text_width,
                                &display_text,
                                label_style.clone(),
                                &label_ellipsis,
                                self.muted_color,
                                Some(&(raw.text, raw.matches)),
                            )
                        });
                        paint_text(ui, text_pos, text_width, self.search_match_background, text);
                    } else {
                        for (i, display_text) in self.texts.labels.iter().enumerate() {
                            let text_pos = Pos2::new(cursor_x, cursor_y);
                            cursor_y += label_height;

                            let label_matches = matches.and_then(|m| m.labels.get(i));
                            let text = self.text_cache.label(i, || {
                                build_paint_text(
                                    ui,
                                    text_width,
                                    display_text,
                                    label_style.clone(),
                                    &label_ellipsis,
                                    self.muted_color,
                                    label_matches,
                                )
                            });
                            paint_text(
                                ui,
                                text_pos,
                                text_width,
                                self.search_match_background,
                                text,
                            );
                        }
                    }

                    if raw.is_none()
                        && self.preview.is_some()
                        && let Some(display_text) = self.texts.preview_source.as_ref()
                    {
                        let text_pos = Pos2::new(cursor_x, cursor_y);

                        let src_matches = matches.and_then(|m| m.preview_source.as_ref());
                        let text = self.text_cache.preview_source(|| {
                            build_paint_text(
                                ui,
                                text_width,
                                display_text,
                                label_style,
                                &label_ellipsis,
                                self.muted_color,
                                src_matches,
                            )
                        });
                        let galley_rect = paint_text(
                            ui,
                            text_pos,
                            text_width,
                            self.search_match_background,
                            text,
                        );

                        // Drawing text underline manually with offset to workaround https://github.com/emilk/egui/issues/5855
                        let text_underline = Stroke {
                            width: 1.0,
                            color: visuals.text_color(),
                        };
                        let underline_y = text_pos.y + galley_rect.height() - text_underline.width
                            + self.underline_offset;
                        ui.painter().line_segment(
                            [
                                Pos2::new(text_pos.x, underline_y),
                                Pos2::new(text_pos.x + galley_rect.width(), underline_y),
                            ],
                            text_underline,
                        );
                    }

                    if let Some(raw) = raw {
                        let text_pos = Pos2::new(
                            cursor_x,
                            rect.shrink2(padding).bottom() - sublabel_height.unwrap_or(0.0),
                        );

                        let text = self.text_cache.sublabel(|| {
                            let display_text = build_display_text(
                                raw.mime,
                                sublabel_style.clone(),
                                self.muted_color,
                            );
                            build_paint_text(
                                ui,
                                text_width,
                                &display_text,
                                sublabel_style,
                                &sublabel_ellipsis,
                                self.muted_color,
                                None,
                            )
                        });
                        paint_text(ui, text_pos, text_width, self.search_match_background, text);
                    } else if let Some(display_text) = self.texts.sublabel.as_ref() {
                        let text_pos = Pos2::new(
                            cursor_x,
                            rect.shrink2(padding).bottom() - sublabel_height.unwrap_or(0.0),
                        );

                        let sublabel_matches = matches.and_then(|m| m.sublabel.as_ref());
                        let text = self.text_cache.sublabel(|| {
                            build_paint_text(
                                ui,
                                text_width,
                                display_text,
                                sublabel_style,
                                &sublabel_ellipsis,
                                self.muted_color,
                                sublabel_matches,
                            )
                        });
                        paint_text(ui, text_pos, text_width, self.search_match_background, text);
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

    pub fn invalidate_text_cache(&mut self) {
        self.text_cache = ButtonTexts::default();
    }
}

fn build_paint_text(
    ui: &Ui,
    text_width: f32,
    display_text: &DisplayText,
    style: TextFormat,
    ellipsis: &Arc<Galley>,
    muted_fg: Color32,
    matches: Option<&(&str, &[u32])>,
) -> PaintText {
    let galley = display_text.text.clone().into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Button,
    );
    let ellipsis_width = ellipsis.size().x;
    let rendered_len = measure_truncation(&galley, text_width, ellipsis_width);

    let (highlight, override_text) = matches
        .map(|(text, matches)| build_highlight(text, matches, rendered_len, style, muted_fg))
        .unwrap_or_else(|| (vec![], None));

    let (galley, display_text) = if let Some(override_text) = &override_text {
        let override_galley = override_text.text.clone().into_galley(
            ui,
            Some(TextWrapMode::Extend),
            f32::INFINITY,
            TextStyle::Button,
        );
        (override_galley, override_text)
    } else {
        (galley, display_text)
    };
    let slice = galley_slice(display_text, &galley, text_width, ellipsis_width);

    let highlight_offset = -galley.rows[0].row.glyphs[slice.start_glyph_idx].pos.x
        + if slice.leading_ellipsis {
            ellipsis_width
        } else {
            0.0
        };
    let highlight_rects = build_highlight_rects(&galley, &highlight, highlight_offset);

    let mut layout_job = LayoutJob::default();
    if slice.leading_ellipsis {
        append_job(&mut layout_job, &ellipsis.job);
    }
    append_job_range(
        &mut layout_job,
        &galley.job,
        slice.start_glyph_idx,
        slice.end_glyph_idx,
    );
    if slice.trailing_ellipsis {
        append_job(&mut layout_job, &ellipsis.job);
    }

    PaintText {
        text: layout_job.into(),
        highlight_rects,
    }
}

fn paint_text(
    ui: &Ui,
    text_pos: Pos2,
    text_width: f32,
    search_match_bg: Color32,
    text: &PaintText,
) -> Rect {
    let visuals = &ui.style().visuals.widgets.inactive;
    let fallback_text_color = visuals.text_color();
    let painter = ui.painter();

    let galley = text.text.clone().into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        text_width,
        TextStyle::Button,
    );
    let galley_rect = Rect::from_min_size(text_pos, galley.size());

    for rect in &text.highlight_rects {
        painter.rect_filled(rect.translate(text_pos.to_vec2()), 0.0, search_match_bg);
    }
    painter.galley(text_pos, galley, fallback_text_color);

    galley_rect
}

fn build_display_text(
    text: &str,
    default_text_format: TextFormat,
    muted_fg: Color32,
) -> DisplayText {
    let mut job_builder = LayoutJobBuilder::new(default_text_format, muted_fg);
    let mut has_trailing = false;

    let (leading_ws_byte_end, trailing_ws_byte_start) = text_whitespace_bounds(text);
    for (char_idx, (display, _, _, muted)) in
        iter_char_to_display_text(text, leading_ws_byte_end, trailing_ws_byte_start).enumerate()
    {
        if char_idx > DISPLAY_LIMIT {
            has_trailing = true;
            break;
        }

        job_builder.push(display, muted);
    }

    DisplayText {
        text: job_builder.finish().into(),
        start_glyph_idx: 0,
        has_leading: false,
        has_trailing,
    }
}

fn display_text_height(ui: &Ui, text_format: TextFormat) -> f32 {
    let mut job = LayoutJob::default();
    job.append(" ", 0.0, text_format);
    let text: WidgetText = job.into();
    text.into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Button,
    )
    .size()
    .y
}

fn measure_truncation(galley: &Galley, text_width: f32, ellipsis_width: f32) -> usize {
    let row = &galley.rows[0].row;

    let mut glyph_len = 0;
    let mut width = 0.0;
    while glyph_len < row.glyphs.len() && width + row.glyphs[glyph_len].advance_width <= text_width
    {
        width += row.glyphs[glyph_len].advance_width;
        glyph_len += 1;
    }

    if glyph_len < row.glyphs.len() {
        while width > text_width - ellipsis_width && glyph_len > 0 {
            glyph_len -= 1;
            width -= row.glyphs[glyph_len].advance_width;
        }
    }

    glyph_len
}

fn build_highlight(
    text: &str,
    matches: &[u32],
    rendered_len: usize,
    style: TextFormat,
    muted_fg: Color32,
) -> (Vec<Range<usize>>, Option<DisplayText>) {
    if matches.is_empty() {
        return (vec![], None);
    }

    let (leading_ws_byte_end, trailing_ws_byte_start) = text_whitespace_bounds(text);
    let mut hl_tracker = HighlightTracker::new(matches, 0);
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
        let (highlight, override_text) = build_override_text(
            text,
            matches,
            style,
            muted_fg,
            leading_ws_byte_end,
            trailing_ws_byte_start,
        );
        (highlight, Some(override_text))
    } else {
        (hl_tracker.finish(), None)
    }
}

fn galley_slice(
    display_text: &DisplayText,
    galley: &Galley,
    text_width: f32,
    ellipsis_width: f32,
) -> GalleySlice {
    let row = &galley.rows[0].row;
    let mut start_glyph_idx = display_text.start_glyph_idx.min(row.glyphs.len());
    let mut render_to_end_width = row.size.x - row.glyphs[start_glyph_idx].pos.x;

    let leading_ellipsis = display_text.has_leading || display_text.start_glyph_idx > 0;
    let leading_ellipsis_width = if leading_ellipsis {
        ellipsis_width
    } else {
        0.0
    };
    let trailing_ellipsis =
        display_text.has_trailing || render_to_end_width > text_width - leading_ellipsis_width;
    let trailing_ellipsis_width = if trailing_ellipsis {
        ellipsis_width
    } else {
        0.0
    };

    // If the galley has empty space at the end, scroll glyphs before the start glyph in to fill it
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

    let mut end_glyph_idx = start_glyph_idx;
    let mut total_render_glyph_width = 0.0;
    for i in start_glyph_idx..row.glyphs.len() {
        let new_total_width = total_render_glyph_width + row.glyphs[i].advance_width;
        if new_total_width > renderable_width {
            break;
        }

        total_render_glyph_width = new_total_width;
        end_glyph_idx = i;
    }

    GalleySlice {
        start_glyph_idx,
        end_glyph_idx,
        leading_ellipsis,
        trailing_ellipsis,
    }
}

// LayoutJob's TextFormat's `background` only highlights each section separatedly, and each
// section can have different height and position, so we need to do the highlighting ourself.
fn build_highlight_rects(galley: &Galley, highlight: &[Range<usize>], x_offset: f32) -> Vec<Rect> {
    let placed_row = &galley.rows[0];
    let row = &placed_row.row;
    let row_top = placed_row.pos.y;
    let row_bottom = row_top + row.size.y;
    let glyph_len = row.glyphs.len();

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
            placed_row.pos.x + row.size.x
        };

        rects.push(
            Rect::from_min_max(egui::pos2(x_min, row_top), egui::pos2(x_max, row_bottom))
                .translate(egui::vec2(x_offset, 0.0)),
        );
    }

    rects
}

fn build_override_text(
    text: &str,
    r#match: &[u32],
    default_text_format: TextFormat,
    muted_fg: Color32,
    leading_ws_byte_end: usize,
    trailing_ws_byte_start: usize,
) -> (Vec<Range<usize>>, DisplayText) {
    let mut start = r#match[0] as usize;
    while !text.is_char_boundary(start) {
        start -= 1;
    }

    for _ in 0..SEARCH_MATCH_CONTEXT_LIMIT {
        if let Some(cb) = prev_char_boundary(text, start) {
            start = cb;
        } else {
            break;
        };
    }

    let tail = &text[start..];

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

    let substr_end = start + end;
    let display_start = if count < DISPLAY_LIMIT {
        let mut display_start = 0;
        while count < DISPLAY_LIMIT {
            if let Some(cb) = prev_char_boundary(text, start) {
                start = cb;
            } else {
                break;
            };

            count += 1;
            display_start += 1;
        }
        display_start
    } else {
        0
    };
    let substr = &text[start..substr_end];

    let has_leading = start > 0;
    let has_trailing = substr_end < text.len();

    let leading_ws_byte_end = leading_ws_byte_end.saturating_sub(start);
    let trailing_ws_byte_start = trailing_ws_byte_start.saturating_sub(start);

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
        DisplayText {
            text: job_builder.finish().into(),
            start_glyph_idx: display_start,
            has_leading,
            has_trailing,
        },
    )
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

fn append_job(dst: &mut LayoutJob, src: &LayoutJob) {
    let offset = dst.text.len();
    dst.text.push_str(&src.text);
    dst.sections
        .extend(src.sections.iter().map(|s| LayoutSection {
            leading_space: s.leading_space,
            byte_range: (s.byte_range.start + offset)..(s.byte_range.end + offset),
            format: s.format.clone(),
        }));
}

fn append_job_range(dst: &mut LayoutJob, src: &LayoutJob, start: usize, end_incl: usize) {
    if end_incl < start {
        return;
    }

    let mut offsets = src
        .text
        .char_indices()
        .map(|(b, _)| b)
        .chain(Some(src.text.len()));
    let bs = offsets.nth(start).unwrap_or(src.text.len());
    let be = offsets.nth(end_incl - start).unwrap_or(src.text.len());

    let offset = dst.text.len();
    dst.text.push_str(&src.text[bs..be]);

    for s in &src.sections {
        if s.byte_range.start >= ByteIndex(be) {
            break;
        }

        let lo = s.byte_range.start.max(ByteIndex(bs));
        let hi = s.byte_range.end.min(ByteIndex(be));
        if lo >= hi {
            continue;
        }

        dst.sections.push(LayoutSection {
            leading_space: if s.byte_range.start >= ByteIndex(bs) {
                s.leading_space
            } else {
                0.0
            },
            byte_range: (lo - bs + offset)..(hi - bs + offset),
            format: s.format.clone(),
        });
    }
}

struct HighlightTracker<'a> {
    matches: Peekable<Iter<'a, u32>>,
    byte_origin: usize,
    highlights: Vec<Range<usize>>,
    start_char_idx: Option<usize>,
    display_len: usize,
}

impl<'a> HighlightTracker<'a> {
    fn new(matches: &'a [u32], byte_origin: usize) -> Self {
        Self {
            matches: matches.iter().peekable(),
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
        while let Some(&&matched_byte) = self.matches.peek()
            && (start..end).contains(&(matched_byte as usize))
        {
            highlighted = true;
            self.matches.next();
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

#[derive(Debug)]
struct DisplayText {
    text: WidgetText,
    start_glyph_idx: usize,
    has_leading: bool,
    has_trailing: bool,
}

#[derive(Debug)]
struct GalleySlice {
    start_glyph_idx: usize,
    end_glyph_idx: usize,
    leading_ellipsis: bool,
    trailing_ellipsis: bool,
}

#[derive(Debug, Default)]
pub struct ClipboardButtonState<'a> {
    is_active: bool,
    is_pinned: bool,
    keyboard_hint: Option<&'static str>,
    matches: Option<ButtonTexts<(&'a str, &'a [u32])>>,
    raw: Option<RawMatch<'a>>,
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
    pub fn matches(mut self, matches: ButtonTexts<(&'a str, &'a [u32])>) -> Self {
        self.matches = Some(matches);
        self
    }

    #[inline]
    pub fn raw(mut self, raw: RawMatch<'a>) -> Self {
        self.raw = Some(raw);
        self
    }
}

#[derive(Debug)]
struct PaintText {
    text: WidgetText,
    highlight_rects: Vec<Rect>,
}

#[derive(Debug)]
pub struct RawMatch<'a> {
    pub text: &'a str,
    pub matches: &'a [u32],
    pub mime: &'a str,
}

#[derive(Debug)]
pub struct ButtonTexts<T> {
    pub labels: Vec<T>,
    pub sublabel: Option<T>,
    pub preview_source: Option<T>,
}

impl<T> Default for ButtonTexts<T> {
    fn default() -> Self {
        Self {
            labels: Default::default(),
            sublabel: Default::default(),
            preview_source: Default::default(),
        }
    }
}

impl<T> ButtonTexts<T> {
    fn label(&mut self, i: usize, build: impl FnOnce() -> T) -> &T {
        if self.labels.len() <= i {
            self.labels.push(build());
        }
        &self.labels[i]
    }

    fn sublabel(&mut self, build: impl FnOnce() -> T) -> &T {
        self.sublabel.get_or_insert_with(build)
    }

    fn preview_source(&mut self, build: impl FnOnce() -> T) -> &T {
        self.preview_source.get_or_insert_with(build)
    }
}
