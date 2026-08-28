use crate::{
    utils::{is_letter_keysym, keysym_to_egui_key},
    x11_key_converter::X11KeyConverter,
    x11_window::X11Window,
};
use anyhow::Result;
use egui::{
    Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, TouchPhase, Vec2,
};
use log::trace;
use x11rb::protocol::{
    Event as X11Event,
    xproto::{ConnectionExt as _, KeyButMask},
};
use xkeysym::Keysym;

pub struct Input<'a> {
    pub egui_input: RawInput,
    window: &'a X11Window<'a>,
    key_converter: &'a X11KeyConverter<'a>,
    modifiers: Modifiers,
}

impl<'a> Input<'a> {
    pub fn new(window: &'a X11Window, key_converter: &'a X11KeyConverter) -> Result<Self> {
        let egui_input = RawInput {
            focused: true,
            screen_rect: Some(Rect::from_min_size(
                Pos2::new(0.0, 0.0),
                Vec2::new(window.dimensions.width as _, window.dimensions.height as _),
            )),
            ..Default::default()
        };

        Ok(Input {
            egui_input,
            window,
            key_converter,
            modifiers: Modifiers::default(),
        })
    }

    pub fn handle_event(&mut self, event: &X11Event) {
        let modifiers = &mut self.modifiers;

        let egui_event = match event {
            X11Event::ButtonPress(ev) | X11Event::ButtonRelease(ev) if ev.detail <= 3 => {
                let pressed = matches!(event, X11Event::ButtonPress(_));
                let pointer_button = match ev.detail {
                    1 => Some(PointerButton::Primary),
                    2 => Some(PointerButton::Middle),
                    3 => Some(PointerButton::Secondary),
                    _ => None,
                };

                let (x, y) = self.window.get_current_win_pos();
                let rel_pos = Pos2::new((ev.root_x - x) as f32, (ev.root_y - y) as f32);
                trace!(
                    "pointer button: {pointer_button:?}, pressed={pressed}, root=({}, {}), relative=({}, {})",
                    ev.root_x, ev.root_y, rel_pos.x, rel_pos.y
                );
                pointer_button.map(|button| Event::PointerButton {
                    pos: rel_pos,
                    button,
                    pressed,
                    modifiers: *modifiers,
                })
            }
            X11Event::ButtonPress(ev) | X11Event::ButtonRelease(ev) => {
                let delta = match ev.detail {
                    4 => Some(egui::vec2(0.0, 1.0)),
                    5 => Some(egui::vec2(0.0, -1.0)),
                    6 => Some(egui::vec2(1.0, 0.0)),
                    7 => Some(egui::vec2(-1.0, 0.0)),
                    _ => None,
                };

                trace!("mouse wheel delta: {delta:?}");
                delta.map(|d| Event::MouseWheel {
                    unit: MouseWheelUnit::Line,
                    delta: d,
                    modifiers: *modifiers,
                    phase: TouchPhase::Move,
                })
            }
            X11Event::KeyPress(ev) | X11Event::KeyRelease(ev) => 'blk: {
                let pressed = matches!(event, X11Event::KeyPress(_));
                let keycode = ev.detail;

                if let Some(keysym) = self.key_converter.keycode_to_keysym(keycode.into()) {
                    if keysym.is_modifier_key() {
                        let mut modifiers_updated = false;
                        if keysym == Keysym::Alt_L || keysym == Keysym::Alt_R {
                            modifiers_updated = true;
                            modifiers.alt = pressed;
                        }
                        if keysym == Keysym::Control_L || keysym == Keysym::Control_R {
                            modifiers_updated = true;
                            modifiers.ctrl = pressed;
                        }
                        if keysym == Keysym::Shift_L || keysym == Keysym::Shift_R {
                            modifiers_updated = true;
                            modifiers.shift = pressed;
                        }

                        break 'blk if modifiers_updated {
                            trace!("modifiers updated: {modifiers:?}");
                            Some(Event::ModifiersChanged(*modifiers))
                        } else {
                            trace!("ignoring modifier: {keysym:?}");
                            None
                        };
                    }

                    if let Some(key) = keysym_to_egui_key(Keysym::new(keysym.into())) {
                        trace!(
                            "key: {key:?}, pressed={pressed}, keysym={keysym:?}, keycode={keycode}"
                        );

                        let caps_locked = ev.state.contains(KeyButMask::LOCK);
                        let modifiers = if caps_locked && is_letter_keysym(keysym) {
                            let mut modifiers_with_caps = *modifiers;
                            modifiers_with_caps.shift = !modifiers_with_caps.shift;
                            modifiers_with_caps
                        } else {
                            *modifiers
                        };

                        break 'blk Some(Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false, // egui will fill this in for us!
                            modifiers,
                        });
                    } else {
                        trace!("unknown keysym: {keysym:?}");
                    }
                } else {
                    trace!("unknown keycode: {keycode}");
                }

                None
            }
            X11Event::MotionNotify(ev) => {
                let (x, y) = self.window.get_current_win_pos();
                let rel_pos = Pos2::new((ev.root_x - x) as f32, (ev.root_y - y) as f32);
                trace!(
                    "pointer moved: root=({}, {}), relative=({}, {})",
                    ev.root_x, ev.root_y, rel_pos.x, rel_pos.y
                );
                Some(Event::PointerMoved(rel_pos))
            }
            _ => None,
        };

        if let Some(egui_event) = egui_event {
            self.egui_input.events.push(egui_event);
        }
    }

    pub fn update_pointer_pos(&mut self) -> Result<()> {
        let pointer = self
            .window
            .conn
            .query_pointer(self.window.screen.root)?
            .reply()?;

        let (x, y) = self.window.get_current_win_pos();
        let rel_pos = Pos2::new((pointer.root_x - x) as f32, (pointer.root_y - y) as f32);
        trace!(
            "start tracking pointer: root=({}, {}), relative=({}, {})",
            pointer.root_x, pointer.root_y, rel_pos.x, rel_pos.y
        );
        self.egui_input.events.push(Event::PointerMoved(rel_pos));

        Ok(())
    }

    pub fn reset_state(&mut self) {
        self.modifiers = Modifiers::default();
    }
}
