use std::{
    collections::HashMap,
    ffi::CString,
    fs::{self, File},
    io::Read as _,
    mem,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr as _,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use egui::{
    Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak,
    FullOutput, Id, LayerId, Order, Pos2, RawInput, Rect, RichText, Stroke, TextureHandle,
    TextureOptions, Vec2,
    emath::GuiRounding as _,
    epaint,
    scroll_area::{DragScroll, ScrollSource},
};
use fontconfig::Fontconfig;
use image::{GenericImageView, RgbaImage};
use log::{debug, error, info, log_enabled, trace, warn};
use xdg_mime::SharedMimeInfo;

use crate::{
    AppMode, ScrollAreaStateExt,
    color::parse_color,
    config::{Config, Dimensions, LayoutConfig, ThemeConfig},
    ext::RectExt as _,
    freedesktop_cache::get_cached_thumbnail,
    keymap_action::{KeyChord, ScrollAction},
    ordered_hash_map::OrderedHashMapView,
    selection_item::{self, ActedOnUris, MozUrl, SelectionItem},
    utils::is_image_mime,
    widgets::{clipboard_button::ClipboardButton, help_modal::HelpModal},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFlow {
    TopToBottom,
    BottomToTop,
}

impl UiFlow {
    pub fn flipped(self) -> Self {
        use UiFlow::*;
        match self {
            TopToBottom => BottomToTop,
            BottomToTop => TopToBottom,
        }
    }
}

struct ImageInfo {
    r#type: String,
    thumbnail: RgbaImage,
    size: Option<(u32, u32)>,
}

const ERROR_MESSAGE_TIMEOUT: Duration = Duration::from_secs(1);

const FALLBACK_IMG_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/images/fallback_image.png"
));
const FALLBACK_FILE_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/images/fallback_file.png"
));
const FALLBACK_DIR_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/images/fallback_directory.png"
));
const FALLBACK_UNKNOWN_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/images/fallback_unknown.png"
));
struct Fallback {
    image: RgbaImage,
    file: RgbaImage,
    directory: RgbaImage,
    unknown: RgbaImage,
}

const NOTO_SANS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/Noto_Sans/NotoSans-Regular.ttf"
));
const NOTO_MATH: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/Noto_Sans_Math/NotoSansMath-Regular.ttf"
));
const NOTO_EMOJI: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/Noto_Emoji/NotoEmoji-Regular.ttf"
));

#[derive(Debug)]
struct ScrollAreaInfo {
    id: Option<egui::Id>,
    content_size: f32,
    inner_rect: Rect,
    offset: f32,
    is_scrolling: bool,
}

#[derive(Debug)]
struct PrevPass {
    scroll_output: ScrollAreaInfo,
    item_widgets: HashMap<u64, (egui::Id, Rect)>,
}

#[derive(Debug)]
struct UiState {
    pointer_acted: bool,
    pointer_pos: Pos2,
    hovered_item: Option<u64>,
    scroll_bar_hidden: bool,
    removed_item_rect: Option<Rect>,
}

pub struct Ui<'a> {
    pub egui_ctx: Rc<egui::Context>,
    config: &'a Config,
    fonts: FontDefinitions,
    button_widgets: HashMap<u64, ClipboardButton>,
    fallback: Fallback,
    help_modal: HelpModal,
    color_preview_background_texture: TextureHandle,
    error_message: Option<(String, Instant)>,

    is_initial_run: bool,
    prev_active_id: u64,
    prev_active_idx: usize,
    prev_flow: Option<UiFlow>,
    reset_scroll_offset_next_run: bool,
    prev_pass: PrevPass,
    state: UiState,
}

impl<'a> Ui<'a> {
    pub fn new(config: &'a Config) -> Result<Self> {
        info!("creating egui context");
        let egui_ctx = Self::create_egui_context(config);
        let font = &config.font;
        let mut fonts = FontDefinitions::default();

        info!("setting default fonts");
        fonts.font_data.insert(
            "NotoSans-Regular".to_owned(),
            Arc::new(FontData::from_static(NOTO_SANS)),
        );
        fonts.font_data.insert(
            "NotoEmoji-Regular".to_owned(),
            Arc::new(FontData::from_static(NOTO_EMOJI).tweak(FontTweak {
                scale: 0.81,
                ..Default::default()
            })),
        );
        // Mostly for the newline symbol (↵) and the tab symbol (⇥)
        fonts.font_data.insert(
            "NotoSansMath-Regular".to_owned(),
            Arc::new(FontData::from_static(NOTO_MATH)),
        );

        let mut font_family_names = vec![];

        if !font.families.is_empty() {
            info!("setting custom fonts")
        }
        for (i, font_family) in font.families.iter().enumerate() {
            if let Some(font_path) = Self::find_font(font_family)? {
                debug!("found font family '{font_family}' file: {font_path:?}");
                fonts.font_data.insert(
                    font_family.clone(),
                    Arc::new(FontData::from_owned(fs::read(font_path)?).tweak(FontTweak {
                        y_offset_factor: *font.y_offset_factors.get(i).unwrap_or(&0.0),
                        ..Default::default()
                    })),
                );

                font_family_names.push(font_family.clone());
            } else {
                warn!("font family '{font_family}' not found");
            }
        }

        font_family_names.push("NotoSans-Regular".to_owned());
        font_family_names.push("NotoEmoji-Regular".to_owned());
        font_family_names.push("NotoSansMath-Regular".to_owned());

        fonts
            .families
            .insert(FontFamily::Proportional, font_family_names);
        egui_ctx.set_fonts(fonts.clone());

        debug!("loading fallback images");
        let fallback_img = image::load_from_memory(FALLBACK_IMG_BYTES)?.to_rgba8();
        let fallback_file = image::load_from_memory(FALLBACK_FILE_BYTES)?.to_rgba8();
        let fallback_dir = image::load_from_memory(FALLBACK_DIR_BYTES)?.to_rgba8();
        let fallback_unknown = image::load_from_memory(FALLBACK_UNKNOWN_BYTES)?.to_rgba8();

        let color_preview_background_image = make_alpha_checkerboard_image(6);
        let color_preview_background_texture = egui_ctx.load_texture(
            "color_preview_background",
            color_preview_background_image,
            TextureOptions::NEAREST,
        );

        Ok(Ui {
            egui_ctx: Rc::new(egui_ctx),
            config,
            fonts,
            button_widgets: HashMap::new(),
            fallback: Fallback {
                image: fallback_img,
                file: fallback_file,
                directory: fallback_dir,
                unknown: fallback_unknown,
            },
            help_modal: HelpModal::new(),
            color_preview_background_texture,
            error_message: None,

            is_initial_run: true,
            prev_active_id: 0,
            prev_active_idx: 0,
            prev_flow: None,
            reset_scroll_offset_next_run: false,
            prev_pass: PrevPass {
                scroll_output: ScrollAreaInfo {
                    id: None,
                    content_size: 0.0,
                    inner_rect: Rect::ZERO,
                    offset: 0.0,
                    is_scrolling: false,
                },
                item_widgets: HashMap::new(),
            },
            state: UiState {
                pointer_acted: false,
                pointer_pos: Pos2::ZERO,
                hovered_item: None,
                scroll_bar_hidden: config.scroll_bar_auto_hide,
                removed_item_rect: None,
            },
        })
    }

    pub fn reset_context(&mut self) {
        info!("recreating egui context");
        let egui_ctx = Self::create_egui_context(self.config);
        egui_ctx.set_fonts(self.fonts.clone());
        self.egui_ctx = Rc::new(egui_ctx);

        debug!("clearing button widgets");
        self.button_widgets.clear();
    }

    fn create_egui_context(config: &Config) -> egui::Context {
        let egui_ctx = egui::Context::default();
        let layout = &config.layout;
        let font = &config.font;
        let theme = &config.theme;

        info!("setting global egui style");
        egui_ctx.global_style_mut(|style| {
            // style.debug.debug_on_hover = true;
            style.spacing.button_padding = layout.button_padding.into();
            style.spacing.item_spacing = egui::vec2(0.0, layout.button_spacing);
            style.interaction.selectable_labels = false;

            style.visuals.window_fill = theme.background.into();
            style.visuals.window_stroke.color = theme.muted_foreground.into();
            style.visuals.widgets.noninteractive.bg_stroke.color = theme.muted_foreground.into();
            style.visuals.code_bg_color = theme.button_background.into();

            style.visuals.override_text_color = Some(theme.foreground.into());
            for widget in [
                &mut style.visuals.widgets.inactive,
                &mut style.visuals.widgets.hovered,
                &mut style.visuals.widgets.active,
            ] {
                widget.fg_stroke.color = theme.foreground.into();
                widget.weak_bg_fill = theme.button_background.into();
                widget.corner_radius = CornerRadius::same(layout.button_corner_radius);
                widget.bg_stroke = Stroke::NONE;
                widget.expansion = 0.0;
            }
            style.visuals.widgets.active.weak_bg_fill = theme.button_active_background.into();

            for text_style in [egui::TextStyle::Body, egui::TextStyle::Button] {
                if let Some(font_id) = style.text_styles.get_mut(&text_style) {
                    *font_id = egui::FontId::proportional(font.size);
                }
            }
        });

        info!("setting global egui options");
        egui_ctx.options_mut(|options| {
            // Keep search input to always be focused in search mode
            options.input_options.surrender_focus_on = egui::SurrenderFocusOn::Never;
        });

        egui_ctx
    }

    fn find_font(font_family: &str) -> Result<Option<PathBuf>> {
        let fc = Fontconfig::new().ok_or(anyhow!("failed to initialize fontconfig"))?;

        let mut pat = fontconfig::Pattern::new(&fc);
        let family = CString::new(font_family)?;
        pat.add_string(fontconfig::FC_FAMILY, &family);
        pat.add_integer(fontconfig::FC_WEIGHT, fontconfig::FC_WEIGHT_REGULAR);
        pat.add_integer(fontconfig::FC_SLANT, fontconfig::FC_SLANT_ROMAN);
        pat.add_integer(fontconfig::FC_WIDTH, fontconfig::FC_WIDTH_NORMAL);
        let fonts = fontconfig::list_fonts(&pat, None);

        if log_enabled!(log::Level::Trace) {
            trace!("found all fonts with pattern {pat:?}:");
            fonts.print();
        }

        let font = fonts.iter().next();
        Ok(font.and_then(|f| f.filename().map(PathBuf::from)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        egui_input: RawInput,
        mode: AppMode,
        flow: UiFlow,
        selection_items: &OrderedHashMapView<u64, SelectionItem>,
        scroll_actions: &[ScrollAction],
        active_id: &mut u64,
        pending_keys: &mut Vec<KeyChord>,
        search_query: &mut String,
    ) -> Result<(FullOutput, Option<u64>)> {
        trace!("painting ui with flow {flow:?}");

        let egui_ctx = Rc::clone(&self.egui_ctx);
        let original_active_id = *active_id;
        let active_idx = selection_items.iter().position(|(id, _)| *id == *active_id);
        let active_item_removed = active_idx.is_none();
        let prev_active_rect = self.prev_pass.item_widgets.get(active_id).map(|(_, r)| r);
        let is_search_empty = search_query.is_empty();

        self.state.removed_item_rect = if active_item_removed {
            prev_active_rect.cloned()
        } else {
            None
        };

        self.process_scroll_actions(active_id, active_idx, selection_items, scroll_actions, flow);
        self.process_pointer_events(&egui_input.events);

        if selection_items.len() != self.prev_pass.item_widgets.len() {
            egui_ctx.request_discard(
                "Recalculate various stuffs that are based on content size. \
                 Fast path of the `request_discard` call in `scroll_area`, \
                 helps with skipping some calculations that use `will_discard`",
            );
        }

        if let Some(active_idx) = active_idx
            && active_idx != self.prev_active_idx
        {
            egui_ctx.request_discard("Recalculate scroll offset when active item is moved");
        }

        let mut run_error = None;
        let mut clicked_item = None;
        let full_output = egui_ctx.run_ui(egui_input, |ui| {
            // Pick new active item if the current one got removed
            if !ui.will_discard() && active_item_removed {
                *active_id = self
                    .pick_new_item_from_removed(selection_items)
                    .or_else(|| selection_items.get_by_index(0).map(|(id, _)| *id))
                    .unwrap_or(0);
            }

            // Update active item using hovered item
            let mut active_id_updated_by_hovering = false;
            if !ui.will_discard()
                && !self.is_initial_run
                && let Some(hovered_id) = self.get_active_item_from_hovering(ui, *active_id)
            {
                *active_id = hovered_id;
                active_id_updated_by_hovering = true;
            }

            // Active item is scrolled out of view, pick a new one
            if !ui.will_discard()
                && !self.is_initial_run
                && self.prev_active_id == *active_id
                && active_idx.is_none_or(|ai| self.prev_active_idx == ai)

                // If the pointer is on the top item (A) and the user scrolls up, an item (B)
                // scrolls in that is only partially visible. Without this check, the code below
                // picks A as the active item, but on the next frame hovering over B switches it
                // back, causing flickering until B is fully in view.
                && !active_id_updated_by_hovering

                && let Some(in_view_id) = self.pick_new_item_from_out_of_view(*active_id, flow, selection_items)
            {
                *active_id = in_view_id;
            }

            self.search_panel(ui, mode == AppMode::Search, search_query);

            let scroll_output = egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ui, |ui| {
                    self.ribbon(ui);

                    self.scroll_area(ui, flow, *active_id, |sf, ui| -> Result<()> {
                        sf.prev_pass.item_widgets.clear();

                        if selection_items.is_empty() {
                            let message = if mode == AppMode::Search && !is_search_empty {
                                "No matching items."
                            } else {
                                "Your clipboard history will appear here."
                            };
                            ui.centered_and_justified(|ui| ui.add(egui::Label::new(message)));
                            return Ok(());
                        }

                        let item_it: Box<dyn Iterator<Item = _>> = if flow == UiFlow::BottomToTop {
                            Box::new(selection_items.iter().enumerate().rev())
                        } else {
                            Box::new(selection_items.iter().enumerate())
                        };
                        for (i, (&id, item)) in item_it {
                            let is_active = id == *active_id;
                            let is_pinned = item.is_pinned();

                            let mut btn_widget = sf
                                .button_widgets
                                .get(&item.id())
                                .ok_or_else(|| anyhow!("missing button widget for item {}", item.id()))?
                                .clone()
                                .is_active(is_active)
                                .is_pinned(is_pinned);
                            if sf.config.show_quick_paste_hint && i < 10 {
                                btn_widget = btn_widget.keyboard_hint(((i + 1) % 10).to_string());
                            }

                            let btn = ui.push_id(id, |ui| ui.add(btn_widget)).inner;
                            sf.prev_pass
                                .item_widgets
                                .insert(item.id(), (btn.id, btn.rect));
                        }

                        Ok(())
                    })
                }).inner;

            if let Err(err) = scroll_output {
                run_error = Some(err);
                return;
            }

            clicked_item = ui.viewport(|vp| {
                vp.interact_widgets.clicked.and_then(|id| {
                    self.prev_pass
                        .item_widgets
                        .iter()
                        .find(|&(_, &(widget_id, _))| widget_id == id)
                        .map(|(&id, _)| id)
                })
            });

            if mode == AppMode::Help {
                self.help_modal.show(ui, self.config.layout.window_dimensions.into());
            } else {
                self.help_modal.hide();
            }

            if original_active_id != *active_id && !pending_keys.is_empty() {
                debug!("active item changed, clearing pending keys");
                pending_keys.clear();
            }
            self.pending_keys_overlay(ui, pending_keys);

            self.error_overlay(ui);
        });

        self.is_initial_run = false;
        self.prev_active_id = *active_id;
        self.prev_active_idx = selection_items
            .iter()
            .position(|(id, _)| *id == *active_id)
            .unwrap_or(0);
        self.prev_flow = Some(flow);
        self.reset_scroll_offset_next_run = false;

        match run_error {
            None => Ok((full_output, clicked_item)),
            Some(err) => Err(err),
        }
    }

    fn process_scroll_actions(
        &self,
        active_id: &mut u64,
        active_idx: Option<usize>,
        selection_items: &OrderedHashMapView<u64, SelectionItem>,
        scroll_actions: &[ScrollAction],
        flow: UiFlow,
    ) {
        if scroll_actions.is_empty() {
            return;
        }

        let items_size = selection_items.len();
        if items_size == 0 {
            return;
        }

        let Some(active_idx) = active_idx else {
            warn!(
                "selection item {active_id} is missing from displayed item list, ignoring scroll actions: {scroll_actions:?}"
            );
            return;
        };

        let item_rects = &self.prev_pass.item_widgets;
        let scroll_rect_height = self.prev_pass.scroll_output.inner_rect.height();
        let id_from_idx = |idx| *selection_items.get_by_index(idx).unwrap().0;
        for action in scroll_actions {
            let action = if flow == UiFlow::TopToBottom {
                *action
            } else {
                action.flipped()
            };

            let next_id = match action {
                ScrollAction::ItemUp => id_from_idx((active_idx + items_size - 1) % items_size),
                ScrollAction::ItemDown => id_from_idx((active_idx + 1) % items_size),

                ScrollAction::HalfUp if active_idx == 0 => id_from_idx(items_size - 1),
                ScrollAction::HalfUp => find_item_at_distance_from(
                    active_idx,
                    -scroll_rect_height / 2.0,
                    selection_items,
                    item_rects,
                ),
                ScrollAction::HalfDown if active_idx == items_size - 1 => id_from_idx(0),
                ScrollAction::HalfDown => find_item_at_distance_from(
                    active_idx,
                    scroll_rect_height / 2.0,
                    selection_items,
                    item_rects,
                ),

                ScrollAction::PageUp if active_idx == 0 => id_from_idx(items_size - 1),
                ScrollAction::PageUp => find_item_at_distance_from(
                    active_idx,
                    -scroll_rect_height,
                    selection_items,
                    item_rects,
                ),
                ScrollAction::PageDown if active_idx == items_size - 1 => id_from_idx(0),
                ScrollAction::PageDown => find_item_at_distance_from(
                    active_idx,
                    scroll_rect_height,
                    selection_items,
                    item_rects,
                ),

                ScrollAction::ToTop => id_from_idx(0),
                ScrollAction::ToBottom => id_from_idx(items_size - 1),
            };

            *active_id = next_id;
        }
    }

    fn process_pointer_events(&mut self, events: &[egui::Event]) {
        self.state.pointer_acted = false;
        for ev in events {
            if matches!(
                ev,
                egui::Event::PointerMoved(_)
                    | egui::Event::MouseWheel { .. }
                    | egui::Event::PointerButton { .. }
            ) {
                self.state.pointer_acted = true;
            }

            if let egui::Event::PointerMoved(pointer_pos) = ev {
                self.state.pointer_pos = *pointer_pos;
            }
        }
    }

    fn pick_new_item_from_removed(
        &self,
        selection_items: &OrderedHashMapView<u64, SelectionItem>,
    ) -> Option<u64> {
        if selection_items.is_empty() {
            return None;
        }

        if selection_items.len() == 1 {
            return Some(*selection_items.get_by_index(0).unwrap().0);
        }

        let removed_rect = self.state.removed_item_rect?;
        selection_items
            .iter()
            .filter_map(|(id, _)| {
                self.prev_pass
                    .item_widgets
                    .get(id)
                    .map(|(_, rect)| (*id, (rect.center().y - removed_rect.center().y).abs()))
            })
            .min_by(|(_, dist1), (_, dist2)| dist1.total_cmp(dist2))
            .map(|(id, _)| id)
    }

    fn get_active_item_from_hovering(&mut self, ui: &egui::Ui, active_id: u64) -> Option<u64> {
        if self.state.hovered_item.is_some_and(|hi| hi == active_id) || self.state.pointer_acted {
            let hovered_item = ui.viewport(|vp| {
                self.prev_pass
                    .item_widgets
                    .iter()
                    .find(|(_, (widget_id, _))| vp.interact_widgets.hovered.contains(widget_id))
            });
            if let Some((&hovered_item_id, _)) = hovered_item {
                self.state.hovered_item = Some(hovered_item_id);
            } else {
                self.state.hovered_item = None;
            }
        } else {
            self.state.hovered_item = None;
        }

        self.state.hovered_item
    }

    fn pick_new_item_from_out_of_view(
        &self,
        active_id: u64,
        flow: UiFlow,
        selection_items: &OrderedHashMapView<u64, SelectionItem>,
    ) -> Option<u64> {
        if let Some((_, active_rect)) = self.prev_pass.item_widgets.get(&active_id)
            && let scroll_rect = self
                .prev_pass
                .scroll_output
                .inner_rect
                .shrink2(egui::vec2(0.0, self.config.layout.window_padding.y as f32))
            && !scroll_rect.contains_rect(*active_rect)
        {
            let active_rect_above_view = active_rect.min.y < scroll_rect.min.y;
            #[allow(clippy::collapsible_else_if)]
            let near_idx_offset = if flow == UiFlow::TopToBottom {
                if active_rect_above_view { 0 } else { 1 }
            } else {
                if active_rect_above_view { 1 } else { 0 }
            };

            let found_idx = selection_items.binary_search_by(|(k, _)| {
                let rect = self
                    .prev_pass
                    .item_widgets
                    .get(k)
                    .map(|(_, r)| r)
                    .unwrap_or(&Rect::ZERO);
                let order = if active_rect_above_view {
                    rect.min.y.total_cmp(&scroll_rect.min.y)
                } else {
                    rect.max.y.total_cmp(&scroll_rect.max.y)
                };

                if flow == UiFlow::TopToBottom {
                    order
                } else {
                    order.reverse()
                }
            });
            let found_idx = match found_idx {
                Ok(exact_idx) => Some(exact_idx),
                Err(near_idx)
                    if near_idx >= near_idx_offset
                        && near_idx - near_idx_offset < selection_items.len() =>
                {
                    Some(near_idx - near_idx_offset)
                }
                Err(_) => None,
            };

            if let Some(found_idx) = found_idx {
                return Some(*selection_items.get_by_index(found_idx).unwrap().0);
            }
        }

        None
    }

    fn scroll_area<R>(
        &mut self,
        ui: &mut egui::Ui,
        flow: UiFlow,
        active_id: u64,
        add_contents: impl FnOnce(&mut Self, &mut egui::Ui) -> R,
    ) -> R {
        let LayoutConfig {
            window_padding: padding,
            scroll_bar_margin,
            ..
        } = self.config.layout;
        let theme = &self.config.theme;
        let drag_scroll = if self.config.drag_scroll {
            DragScroll::Always
        } else {
            DragScroll::Never
        };
        let scroll_bar_rect = egui::Rect::from_min_max(
            ui.min_rect().min + egui::vec2(0.0, scroll_bar_margin),
            ui.max_rect().max - egui::vec2(0.0, scroll_bar_margin),
        );

        let prev_content_size = self.prev_pass.scroll_output.content_size;
        let prev_scroll_rect = self.prev_pass.scroll_output.inner_rect;
        let prev_offset = self.prev_pass.scroll_output.offset;
        let active_prev_rect = self.prev_pass.item_widgets.get(&active_id).map(|(_, r)| r);
        let prev_flow = self.prev_flow;
        let is_prev_scrolling = self.prev_pass.scroll_output.is_scrolling;
        let is_prev_overflow = prev_content_size > prev_scroll_rect.height();

        // With `scroll_bar_auto_hide` = true, on window shown, the scroll bar may still be
        // briefly visible, so we hide it before showing the window. This shows the scroll
        // bar back when the pointer starts to move.
        if self.config.scroll_bar_auto_hide && prev_scroll_rect.contains(self.state.pointer_pos) {
            self.state.scroll_bar_hidden = false;
        }

        let original_style = ui.style().as_ref().clone();
        let mut scrollbar_style = original_style.clone();
        scrollbar_style.visuals.extreme_bg_color = theme.scroll_background.into();
        if self.state.scroll_bar_hidden || !is_prev_overflow {
            scrollbar_style.spacing.scroll.dormant_background_opacity = 0.0;
            scrollbar_style.spacing.scroll.dormant_handle_opacity = 0.0;
            scrollbar_style.spacing.scroll.active_background_opacity = 0.0;
            scrollbar_style.spacing.scroll.active_handle_opacity = 0.0;
        } else if !self.config.scroll_bar_auto_hide {
            scrollbar_style.spacing.scroll.dormant_background_opacity =
                scrollbar_style.spacing.scroll.active_background_opacity;
            scrollbar_style.spacing.scroll.dormant_handle_opacity =
                scrollbar_style.spacing.scroll.active_handle_opacity;
        }
        ui.set_style(scrollbar_style);

        let mut scroll_area = egui::ScrollArea::vertical()
            .id_salt("main_scroll_area")
            .auto_shrink(false)
            .scroll_source(ScrollSource {
                drag: drag_scroll,
                ..Default::default()
            })
            .scroll_bar_rect(scroll_bar_rect);

        let mut forced_scroll_offset = None;

        if self.is_initial_run || self.reset_scroll_offset_next_run {
            if self.is_initial_run {
                debug!("resetting scroll offset on initial run");
            } else if self.reset_scroll_offset_next_run {
                debug!("resetting scroll offset on demand");
            }

            if flow == UiFlow::TopToBottom {
                forced_scroll_offset = Some(0.0);
            }
            if flow == UiFlow::BottomToTop && is_prev_overflow {
                forced_scroll_offset = Some(prev_content_size - prev_scroll_rect.height());
            }
        }

        // Set correct scroll offset when an item is removed right in the first pass,
        // so `pick_new_item_from_removed` can pick correct item in the next one
        let removed_item_height = if ui.will_discard()
            && let Some(rect) = self.state.removed_item_rect
        {
            rect.height() + self.config.layout.button_spacing
        } else {
            0.0
        };
        // `removed_item_height` can contain excess `button_spacing` if the last item is removed
        let next_content_size = (prev_content_size - removed_item_height).max(0.0);

        // Force the content to be bottom-aligned when running with BottomToTop flow
        if flow == UiFlow::BottomToTop && !is_prev_overflow {
            forced_scroll_offset = Some(next_content_size - prev_scroll_rect.height());
        }

        // Force content to be pushed down to fill the removed items' space when at the bottom of the scroll area.
        // This allows picking the correct nearest item for the next active item.
        let next_offset = forced_scroll_offset.unwrap_or(prev_offset);
        if removed_item_height > 0.0 && prev_scroll_rect.height() + next_offset > next_content_size
        {
            debug!("updating scroll offset after item removed");
            forced_scroll_offset = Some(next_content_size - prev_scroll_rect.height());
        }

        // Scroll active item into view if it's goes out of view.
        // During momentum scrolling, the pointer can hover over an item near the edge
        // of the window and make it active. Avoid scrolling that item into view while
        // the list is moving, because that would reset its velocity and stop the scroll.
        let next_offset = forced_scroll_offset.unwrap_or(prev_offset);
        let active_next_rect = active_prev_rect
            .map(|r| {
                if prev_flow == Some(flow.flipped()) {
                    let axis = -prev_offset + prev_content_size / 2.0;
                    r.translate(egui::vec2(0.0, -axis))
                        .flipped_y()
                        .translate(egui::vec2(0.0, axis))
                } else {
                    *r
                }
            })
            .map(|r| r.translate(egui::vec2(0.0, prev_offset - next_offset)));
        if !is_prev_scrolling
            && let Some(active_rect) = active_next_rect
            && let unpadded_scroll_rect =
                prev_scroll_rect.shrink2(egui::vec2(0.0, padding.y as f32))
            && !unpadded_scroll_rect.contains_rect(active_rect)
        {
            debug!("scrolling active item into view");
            if active_rect.top() < unpadded_scroll_rect.top() {
                forced_scroll_offset = Some(active_rect.top() + next_offset - padding.y as f32);
            } else {
                forced_scroll_offset = Some(
                    active_rect.bottom() + next_offset
                        - unpadded_scroll_rect.height()
                        - padding.y as f32,
                );
            }
        }

        if let Some(offset) = forced_scroll_offset {
            scroll_area = scroll_area.vertical_scroll_offset(offset);

            if let Some(prev_scroll_id) = self.prev_pass.scroll_output.id
                && let Err(err) = egui::scroll_area::State::reset_velocity(ui, prev_scroll_id)
            {
                debug!("failed to reset main scroll area velocity: {err}");
            }
        }

        let output = scroll_area.show_viewport(ui, |ui, vp| {
            ui.set_style(original_style);

            // output.state.offset[1] is the next intended offset, not the currently painted offset
            let current_offset = vp.min.y;

            (
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(padding.x, padding.y))
                    .show(ui, |ui| add_contents(self, ui))
                    .inner,
                current_offset,
            )
        });
        let (response, current_offset) = output.inner;

        if output.content_size.y != prev_content_size {
            ui.request_discard("Recalculate various stuffs that are based on content size");
        }

        let rounded_offset = current_offset
            .round_to_pixels(ui.pixels_per_point())
            .round_ui();
        let is_scrolling = self.prev_pass.scroll_output.offset != rounded_offset;
        self.prev_pass.scroll_output = ScrollAreaInfo {
            id: Some(output.id),
            content_size: output.content_size.y,
            inner_rect: output.inner_rect,
            offset: rounded_offset,
            is_scrolling,
        };

        response
    }

    fn ribbon(&self, ui: &egui::Ui) {
        if !self.config.show_ribbon {
            return;
        }

        let size = self.config.layout.ribbon_size;
        let color = self.config.theme.ribbon;

        let mut points = [
            egui::pos2(-size, 0.0),
            egui::pos2(0.0, 0.0),
            egui::pos2(0.0, size),
        ];
        for p in &mut points {
            p.x += ui.min_rect().width();
        }

        ui.painter().add(epaint::Shape::convex_polygon(
            points.to_vec(),
            color,
            Stroke::NONE,
        ));
    }

    fn pending_keys_overlay(&self, ui: &egui::Ui, pending_keys: &[KeyChord]) {
        if pending_keys.is_empty() {
            return;
        }

        let fg_color: Color32 = self.config.theme.pending_keys_foreground.into();
        let bg_color: Color32 = self.config.theme.pending_keys_background.into();
        let padding: Vec2 = self.config.layout.pending_keys_padding.into();
        let margin: Vec2 = self.config.layout.pending_keys_margin.into();

        let label = pending_keys
            .iter()
            .map(KeyChord::to_string)
            .collect::<Vec<_>>()
            .join(" ");

        let rect = ui.input(|i| i.content_rect());
        let painter = ui.layer_painter(LayerId::new(
            Order::Foreground,
            Id::new("pending_keys_overlay"),
        ));

        let galley = painter.layout(
            label,
            FontId::proportional(self.config.font.overlay_text_size),
            fg_color,
            rect.width() - margin.x * 2.0 - padding.x * 2.0,
        );

        let galley_pos = rect.right_bottom() - margin - padding - galley.size();
        let bg_rect = Rect::from_min_size(galley_pos - padding, galley.size() + padding * 2.0);

        painter.rect_filled(
            bg_rect,
            self.config.layout.pending_keys_corner_radius,
            bg_color,
        );
        painter.galley(galley_pos, galley, fg_color);
    }

    pub fn show_error(&mut self, message: impl Into<String>) {
        self.error_message = Some((message.into(), Instant::now()));
    }

    fn error_overlay(&mut self, ui: &egui::Ui) {
        if self
            .error_message
            .as_ref()
            .is_some_and(|(_, shown_at)| shown_at.elapsed() >= ERROR_MESSAGE_TIMEOUT)
        {
            self.error_message = None;
        }

        let Some((message, _)) = &self.error_message else {
            return;
        };

        let fg_color: Color32 = self.config.theme.error_foreground.into();
        let bg_color: Color32 = self.config.theme.error_background.into();
        let padding: Vec2 = self.config.layout.pending_keys_padding.into();
        let margin: Vec2 = self.config.layout.pending_keys_margin.into();

        let rect = ui.input(|i| i.content_rect());
        let painter = ui.layer_painter(LayerId::new(Order::Foreground, Id::new("error_overlay")));

        let galley = painter.layout(
            message.to_owned(),
            FontId::proportional(self.config.font.overlay_text_size),
            fg_color,
            rect.width() - margin.x * 2.0 - padding.x * 2.0,
        );

        let galley_pos = egui::pos2(
            rect.left() + margin.x + padding.x,
            rect.bottom() - margin.y - padding.y - galley.size().y,
        );
        let bg_rect = Rect::from_min_size(galley_pos - padding, galley.size() + padding * 2.0);

        painter.rect_filled(
            bg_rect,
            self.config.layout.pending_keys_corner_radius,
            bg_color,
        );
        painter.galley(galley_pos, galley, fg_color);
    }

    fn search_panel(&self, ui: &mut egui::Ui, display_search: bool, query: &mut String) {
        let layout = &self.config.layout;
        let padding_x = layout.window_padding.x
            + layout
                .button_padding
                .x
                .clamp(i8::MIN as f32, i8::MAX as f32) as i8;
        let padding_y = layout.window_padding.y;

        let input_id = Id::new("search_input");
        if display_search && !ui.memory(|m| m.has_focus(input_id)) {
            ui.memory_mut(|m| m.request_focus(input_id));
        }

        egui::Panel::bottom("search_panel")
            .resizable(false)
            .drag_to_open(false)
            .frame(egui::Frame::new())
            .show_collapsible(ui, &mut display_search.clone(), |ui| {
                egui::TextEdit::singleline(query)
                    .id(input_id)
                    .frame(
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(padding_x, padding_y)),
                    )
                    .desired_width(f32::INFINITY)
                    .show(ui);
            });
    }

    pub fn reset(&mut self) {
        info!("resetting ui states");
        self.is_initial_run = true;
        self.help_modal.hide();
        self.error_message = None;
        self.state = UiState {
            pointer_acted: false,
            pointer_pos: Pos2::ZERO,
            hovered_item: None,
            scroll_bar_hidden: self.config.scroll_bar_auto_hide,
            removed_item_rect: None,
        };
    }

    pub fn reset_scroll_offset(&mut self) {
        self.reset_scroll_offset_next_run = true;
    }

    pub fn build_button_widget(&mut self, item: &SelectionItem) -> Result<()> {
        trace!("building button widget for item {}", item.id());
        let Ui {
            egui_ctx: ctx,
            config,
            fallback,
            ..
        } = self;

        let text_data = item.text_data();
        let mut img_info = None;
        for (mime, data) in item.data() {
            if is_image_mime(mime) {
                let img_type = mime.split(['/', '+']).nth(1).unwrap_or(mime).to_uppercase();
                let img = if img_type == "SVG" {
                    load_svg(data, config.layout.preview_size.into())
                } else {
                    image::load_from_memory(data)
                        .map(|i| (i.to_rgba8(), i.dimensions()))
                        .map_err(anyhow::Error::from)
                };

                img_info = Some(match img {
                    Ok((img, size)) => {
                        let thumbnail = create_thumbnail(&img, config.layout.preview_size.into());
                        ImageInfo {
                            r#type: img_type,
                            size: Some(size),
                            thumbnail,
                        }
                    }
                    Err(err) => {
                        error!(
                            "failed to load image with mime {mime} of item {}: {err}",
                            item.id()
                        );
                        ImageInfo {
                            r#type: img_type,
                            size: None,
                            thumbnail: fallback.image.clone(),
                        }
                    }
                });
            }
        }

        let mut btn = ClipboardButton::default()
            .secondary_foreground(config.theme.muted_foreground)
            .underline_offset(config.font.underline_offset)
            .with_preview_padding(config.layout.button_with_preview_padding)
            .pin_size(config.layout.pin_size)
            .pin_color(config.theme.pin_color)
            .color_preview_size(config.layout.color_preview_size)
            .color_preview_corner_radius(config.layout.color_preview_corner_radius)
            .color_preview_background(self.color_preview_background_texture.clone());

        if let Some(ActedOnUris {
            action,
            uris: files,
        }) = &text_data.files
        {
            let mut path_iter = files.iter();
            if let Some(path) = path_iter.next() {
                btn = btn.append_label(vec![path.display.as_ref().into()]);
            }
            if let Some(path) = path_iter.next() {
                btn = btn.append_label(vec![path.display.as_ref().into()]);
            }
            let more_count = path_iter.count();

            let mut sublabel_text = "".to_owned();
            sublabel_text.push_str(action.as_ref());

            if more_count > 0 {
                if !sublabel_text.is_empty() {
                    sublabel_text.push_str(" | ");
                }
                sublabel_text.push_str(&format!("+{more_count} MORE..."));
            }

            if !sublabel_text.is_empty() {
                btn = btn.sublabel(
                    RichText::new(sublabel_text.to_uppercase()).size(config.font.secondary_size),
                )
            }

            let thumbnail = create_files_thumbnail(
                files,
                config.layout.preview_size,
                &fallback.file,
                &fallback.directory,
                &fallback.unknown,
            );
            let texture = load_texture(ctx, item.id(), &thumbnail);
            btn = btn.preview(texture, config.layout.preview_size);
        } else if let Some(ImageInfo {
            r#type,
            size,
            thumbnail,
        }) = img_info
        {
            let texture = load_texture(ctx, item.id(), &thumbnail);
            let sublabel_text = if let Some(size) = size {
                format!("{} [{}x{}]", r#type, size.0, size.1)
            } else {
                format!("{} [?x?]", r#type)
            };

            btn = btn
                .preview(texture, config.layout.preview_size)
                .sublabel(RichText::new(sublabel_text).size(config.font.secondary_size))
                .preview_background(config.theme.preview_background);

            if let Some(MozUrl { src, alt }) = &text_data.moz_url {
                if !alt.is_empty() {
                    btn = btn.label(build_display_text(alt, &config.theme));
                }
                btn = btn.preview_source(src);
            }
        } else if let Some(text) = &text_data.plain {
            btn = btn.label(build_display_text(text, &config.theme));
            if config.layout.show_color_preview
                && let Some(color) = parse_color(text)
            {
                btn = btn.color_preview(color);
            }
        } else {
            btn = btn.label(vec![
                RichText::new("[unknown]").color(config.theme.muted_foreground),
            ]);
        }

        self.button_widgets.insert(item.id(), btn);
        Ok(())
    }

    pub fn remove_button_widgets<I: IntoIterator<Item = SelectionItem>>(
        &mut self,
        removed_items: I,
    ) {
        for item in removed_items {
            trace!("removing button widget for item {}", item.id());
            self.button_widgets.remove(&item.id());
        }
    }
}

fn find_item_at_distance_from(
    from_idx: usize,
    distance: f32,
    items: &OrderedHashMapView<u64, SelectionItem>,
    item_widgets: &HashMap<u64, (egui::Id, Rect)>,
) -> u64 {
    let items_size = items.len();
    let id_from_idx = |idx| *items.get_by_index(idx).unwrap().0;
    assert!(items_size > 0);
    assert!(from_idx < items_size);

    #[derive(PartialEq)]
    enum Dir {
        Up,
        Down,
    }

    let target_dist = distance.abs();
    let (start, end, dir) = if distance >= 0.0 {
        if from_idx == items_size - 1 {
            return id_from_idx(from_idx);
        }
        (from_idx + 1, items_size - 1, Dir::Down)
    } else {
        if from_idx == 0 {
            return id_from_idx(from_idx);
        }
        (from_idx - 1, 0, Dir::Up)
    };

    let mut to_idx = start;
    let mut total_dist = 0.0;
    let mut current_pos = item_widgets
        .get(&id_from_idx(from_idx))
        .map(|(_, r)| r.center().y)
        .unwrap_or(0.0);
    loop {
        if let Some((_, rect)) = item_widgets.get(&id_from_idx(to_idx)) {
            let prev_pos = current_pos;
            current_pos = rect.center().y;

            let prev_total_dist = total_dist;
            total_dist += (current_pos - prev_pos).abs();

            if total_dist >= target_dist {
                // Use the previous item if it's nearer to target_dist than the current one
                if prev_total_dist > 0.0 && target_dist - prev_total_dist < total_dist - target_dist
                {
                    to_idx = if dir == Dir::Down {
                        to_idx - 1
                    } else {
                        to_idx + 1
                    };
                }
                break;
            }
        }

        if to_idx == end {
            break;
        };
        to_idx = if dir == Dir::Down {
            to_idx + 1
        } else {
            to_idx - 1
        };
    }

    id_from_idx(to_idx)
}

fn create_files_thumbnail(
    files: &[selection_item::Uri],
    size: Dimensions,
    fallback_file: &RgbaImage,
    fallback_dir: &RgbaImage,
    fallback_unknown: &RgbaImage,
) -> RgbaImage {
    let mut thumbnail = RgbaImage::from_pixel(
        size.width.into(),
        size.height.into(),
        image::Rgba([0, 0, 0, 0]),
    );

    let display_count = files.len().min(4);
    if display_count == 0 {
        return thumbnail;
    }

    // relative coordinates of each file thumbnail inside the thumbnail
    static TEMPLATES: &[&[&[f32; 4]]] = &[
        &[&[0.1, 0.1, 0.9, 0.9]],
        &[&[0.1, 0.1, 0.6, 0.6], &[0.4, 0.4, 0.9, 0.9]],
        &[
            &[0.1, 0.1, 0.55, 0.55],
            &[0.45, 0.2, 0.9, 0.65],
            &[0.225, 0.45, 0.675, 0.9],
        ],
        &[
            &[0.15, 0.1, 0.6, 0.55],
            &[0.45, 0.15, 0.9, 0.6],
            &[0.1, 0.4, 0.55, 0.85],
            &[0.4, 0.45, 0.85, 0.9],
        ],
    ];
    let template = TEMPLATES[display_count - 1];

    for i in 0..display_count {
        let item_thumb_temp = template[i];
        let coord = &[
            (item_thumb_temp[0] * size.width as f32).round() as u16,
            (item_thumb_temp[1] * size.height as f32).round() as u16,
            (item_thumb_temp[2] * size.width as f32).round() as u16,
            (item_thumb_temp[3] * size.height as f32).round() as u16,
        ];
        let size = Vec2::new((coord[2] - coord[0]).into(), (coord[3] - coord[1]).into());

        let (item_thumb, fallback_thumb) = if let Some(file) = &files[i].file_path {
            let is_dir = file.is_dir();
            let file_thumb = get_file_thumbnail(file, size, is_dir).unwrap_or_else(|e| {
                error!("failed to get file thumbnail for {}: {e}", file.display());
                None
            });
            let fallback = if is_dir { fallback_dir } else { fallback_file };
            (file_thumb, fallback)
        } else {
            (None, fallback_unknown)
        };

        let scaled_item_thumb =
            create_thumbnail(item_thumb.as_ref().unwrap_or(fallback_thumb), size);
        image::imageops::overlay(
            &mut thumbnail,
            &scaled_item_thumb,
            coord[0].into(),
            coord[1].into(),
        );
    }

    thumbnail
}

fn get_file_thumbnail<P: AsRef<Path>>(
    file: P,
    size_hint: Vec2,
    is_dir: bool,
) -> Result<Option<RgbaImage>> {
    let thumb_path = if is_dir {
        freedesktop_icon::get_icon("folder")
    } else {
        get_cached_thumbnail(&file)
            .unwrap_or_else(|e| {
                warn!(
                    "failed to get cached thumbnail for {:?}: {e}",
                    file.as_ref()
                );
                None
            })
            .or_else(|| {
                get_file_icon_path(&file).unwrap_or_else(|e| {
                    warn!("failed to get icon for {:?}: {e}", file.as_ref());
                    None
                })
            })
    };
    let Some(path) = thumb_path else {
        return Ok(None);
    };

    if let Some(ext) = path.extension()
        && (ext == "png" || ext == "svg")
    {
        let data = fs::read(&path)?;
        if ext == "png" {
            Ok(Some(image::load_from_memory(&data)?.to_rgba8()))
        } else {
            Ok(Some(load_svg(&data, size_hint)?.0))
        }
    } else {
        warn!("unsupported thumbnail file type, expected png or svg: {path:?}");
        Ok(None)
    }
}

fn get_file_icon_path<P: AsRef<Path>>(file: P) -> Result<Option<PathBuf>> {
    static SMI: LazyLock<SharedMimeInfo> = LazyLock::new(SharedMimeInfo::new);

    let file_data = read_first_n_bytes(&file, 10 * 1024 * 1024)
        .inspect_err(|e| {
            warn!(
                "failed to read {:?} to determine a suitable icon, falling back to generic icon: {e}",
                file.as_ref()
            )
        })
        .ok();
    let data_mime = file_data
        .as_ref()
        .and_then(|data| SMI.get_mime_type_for_data(data))
        .map(|(mime, _)| mime);
    let ext_mime = file_data.and_then(|_| {
        file.as_ref()
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| SMI.get_mime_types_from_file_name(name).first().cloned())
    });

    let mime = if let Some(data_mime) = data_mime {
        if let Some(ext_mime) = ext_mime
            && SMI.mime_type_subclass(&ext_mime, &data_mime)
        {
            ext_mime
        } else {
            data_mime
        }
    } else if let Some(ext_mime) = ext_mime {
        ext_mime
    } else {
        mime::Mime::from_str("application/x-generic")?
    };

    for icon_name in SMI.lookup_icon_names(&mime) {
        if let Some(icon) = freedesktop_icon::get_icon(&icon_name) {
            return Ok(Some(icon));
        }
    }

    debug!(
        "icon for {:?} with mime '{}' not found",
        file.as_ref(),
        mime
    );
    Ok(None)
}

fn create_thumbnail(image: &RgbaImage, size: Vec2) -> RgbaImage {
    let orig_w = image.width() as f32;
    let orig_h = image.height() as f32;
    let scale = (size.x / orig_w).min(size.y / orig_h);
    let thumb_w = (orig_w * scale).round() as u32;
    let thumb_h = (orig_h * scale).round() as u32;

    let scaled = image::imageops::thumbnail(image, thumb_w, thumb_h);

    let mut thumbnail =
        RgbaImage::from_pixel(size.x as u32, size.y as u32, image::Rgba([0, 0, 0, 0]));
    let (w, h) = scaled.dimensions();
    let x_offset = (size.x as u32 - w) / 2;
    let y_offset = (size.y as u32 - h) / 2;
    for y in 0..h {
        for x in 0..w {
            let px = scaled.get_pixel(x, y);
            thumbnail.put_pixel(x + x_offset, y + y_offset, *px);
        }
    }

    thumbnail
}

fn load_texture(ctx: &egui::Context, id: u64, img: &RgbaImage) -> TextureHandle {
    let thumb_size = [img.width() as usize, img.height() as usize];
    ctx.load_texture(
        id.to_string(),
        egui::ColorImage::from_rgba_unmultiplied(thumb_size, img.as_flat_samples().as_slice()),
        Default::default(),
    )
}

pub fn load_svg(svg_bytes: &[u8], size_hint: Vec2) -> Result<(RgbaImage, (u32, u32))> {
    use resvg::{
        tiny_skia::Pixmap,
        usvg::{Options, Transform, Tree},
    };

    let rtree = Tree::from_data(svg_bytes, &Options::default())?;
    let source_size = Vec2::new(rtree.size().width(), rtree.size().height());

    let mut scaled_size = source_size;
    scaled_size *= size_hint.x / source_size.x;
    if scaled_size.y > size_hint.y {
        scaled_size *= size_hint.y / scaled_size.y;
    }
    let scaled_size = scaled_size.round();
    let (w, h) = (scaled_size.x as u32, scaled_size.y as u32);

    let mut pixmap =
        Pixmap::new(w, h).ok_or_else(|| anyhow!("failed to create SVG Pixmap of size {w}x{h}"))?;

    resvg::render(
        &rtree,
        Transform::from_scale(w as f32 / source_size.x, h as f32 / source_size.y),
        &mut pixmap.as_mut(),
    );

    Ok((
        RgbaImage::from_raw(
            w,
            h,
            pixmap
                .pixels()
                .iter()
                .map(|p| p.demultiply())
                .flat_map(|p| [p.red(), p.green(), p.blue(), p.alpha()])
                .collect(),
        )
        .ok_or_else(|| anyhow!("failed to create RgbaImage"))?,
        (w, h),
    ))
}

fn make_alpha_checkerboard_image(cell_count_per_line: usize) -> ColorImage {
    let cell_size = 1;
    let size = cell_count_per_line * cell_size;

    let mut pixels = vec![];
    for y in 0..size {
        for x in 0..size {
            let cx = x / cell_size;
            let cy = y / cell_size;
            pixels.push(if (cx + cy) & 1 == 0 {
                Color32::LIGHT_GRAY
            } else {
                Color32::GRAY
            });
        }
    }

    ColorImage::new([size, size], pixels)
}

fn build_display_text(s: &str, theme: &ThemeConfig) -> Vec<RichText> {
    let mut text = vec![];
    let mut chars = s.chars();

    let mut last_non_whitespace = None;
    let mut trailing_whitespace_str = String::new();
    let mut trailing_count = 0;
    while let Some(c) = chars.next_back() {
        trailing_count += 1;
        if c == ' ' {
            trailing_whitespace_str.push('·');
        } else {
            last_non_whitespace = Some(c);
            break;
        }
    }

    let mut str = String::with_capacity(s.len());
    let mut chars = chars.enumerate().peekable();
    let mut is_leading_whitespace = true;
    let mut i_c = chars.next().or_else(|| {
        last_non_whitespace.map(|c| {
            last_non_whitespace = None;
            (0, c)
        })
    });
    while let Some((i, c)) = i_c {
        // Very very long string causes egui to choke on first render, even when we only display it
        // on a single line
        if i == (10_000 - trailing_count) && chars.peek().is_some() {
            if is_leading_whitespace {
                text.push(RichText::new(str).color(theme.muted_foreground));
            }
            text.push("…".into());
            return text;
        }

        if c != ' ' && is_leading_whitespace {
            is_leading_whitespace = false;
            let prev_str = mem::take(&mut str);
            text.push(RichText::new(prev_str).color(theme.muted_foreground));
        }

        match c {
            '\r' => {
                let prev_str = mem::take(&mut str);
                text.push(prev_str.into());
                text.push(RichText::new('␍').color(theme.muted_foreground));
            }
            '\n' => {
                let prev_str = mem::take(&mut str);
                text.push(prev_str.into());
                text.push(RichText::new('↵').color(theme.muted_foreground));
            }
            '\t' => {
                let prev_str = mem::take(&mut str);
                text.push(prev_str.into());
                text.push(RichText::new(" ⇥ ").color(theme.muted_foreground));
            }
            ' ' if is_leading_whitespace => {
                str.push('·');
            }
            _ => str.push(c),
        }

        i_c = chars.next();
        if i_c.is_none()
            && let Some(last_c) = last_non_whitespace
        {
            i_c = Some((i + 1, last_c));
            last_non_whitespace = None;
        }
    }

    text.push(str.into());
    text.push(RichText::new(trailing_whitespace_str).color(theme.muted_foreground));

    text
}

fn read_first_n_bytes<P: AsRef<Path>>(path: P, n: u64) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    File::open(path)?.take(n).read_to_end(&mut buffer)?;
    Ok(buffer)
}
