use std::iter;

use crate::{
    utils::{is_char_key, is_letter_key, keysym_to_egui_key},
    x11_key_converter::X11KeyConverter,
    x11_window::X11Window,
    xim_handler::{XimEvent, XimHandler},
};
use anyhow::Result;
use egui::{
    Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, TouchPhase, Vec2,
};
use log::{debug, trace, warn};
use x11rb::{
    protocol::{
        Event as X11Event,
        xproto::{ConnectionExt as _, KeyButMask},
    },
    xcb_ffi::XCBConnection,
};
use xim::Client as _;
use xkeysym::Keysym;

pub struct Input<'a> {
    pub egui_input: RawInput,
    pub xim_client: xim::x11rb::X11rbClient<&'a XCBConnection>,
    pub xim_handler: XimHandler,
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

        let xim_client = xim::x11rb::X11rbClient::init(&window.conn, window.screen_num, None)?;
        let xim_handler = XimHandler::new(window.win_id.get());

        Ok(Input {
            egui_input,
            xim_client,
            xim_handler,
            window,
            key_converter,
            modifiers: Modifiers::default(),
        })
    }

    pub fn handle_event(&mut self, event: &X11Event, emit_text_events: bool) -> Result<()> {
        if self.xim_client.filter_event(event, &mut self.xim_handler)? {
            trace!("handling xim events {:?}", self.xim_handler.events);
            for xim_event in std::mem::take(&mut self.xim_handler.events) {
                match xim_event {
                    XimEvent::Forward(key_press_event) => {
                        let event = match key_press_event.response_type & 0x7f {
                            x11rb::protocol::xproto::KEY_PRESS_EVENT => {
                                X11Event::KeyPress(key_press_event)
                            }
                            x11rb::protocol::xproto::KEY_RELEASE_EVENT => {
                                X11Event::KeyRelease(key_press_event)
                            }
                            _ => {
                                warn!(
                                    "unexpected XIM forwarded event type {}: {key_press_event:?}",
                                    key_press_event.response_type & 0x7f
                                );
                                continue;
                            }
                        };

                        let egui_events =
                            self.handle_single_event(&event, emit_text_events, false)?;
                        self.egui_input.events.extend(egui_events);
                    }
                    XimEvent::Egui(ime_event) => {
                        self.egui_input.events.push(Event::Ime(ime_event));
                    }
                }
            }
            return Ok(());
        }

        trace!("handling x11 event {event:?}");
        let egui_events = self.handle_single_event(event, emit_text_events, emit_text_events)?;
        self.egui_input.events.extend(egui_events);
        Ok(())
    }

    fn handle_single_event(
        &mut self,
        event: &X11Event,
        emit_text_events: bool,
        forward_xim: bool,
    ) -> Result<Box<dyn Iterator<Item = Event>>> {
        let modifiers = &mut self.modifiers;
        Ok(match event {
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
                Box::new(
                    pointer_button
                        .map(|button| Event::PointerButton {
                            pos: rel_pos,
                            button,
                            pressed,
                            modifiers: *modifiers,
                        })
                        .into_iter(),
                )
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
                Box::new(
                    delta
                        .map(|d| Event::MouseWheel {
                            unit: MouseWheelUnit::Line,
                            delta: d,
                            modifiers: *modifiers,
                            phase: TouchPhase::Move,
                        })
                        .into_iter(),
                )
            }
            X11Event::KeyPress(ev) | X11Event::KeyRelease(ev) => 'blk: {
                if forward_xim && self.xim_handler.connected {
                    trace!("forwarding key event to XIM server: {ev:?}");
                    self.xim_client.forward_event(
                        self.xim_handler.im_id,
                        self.xim_handler.ic_id,
                        xim::ForwardEventFlag::empty(),
                        ev,
                    )?;
                    break 'blk Box::new(iter::empty());
                }

                let pressed = matches!(event, X11Event::KeyPress(_));
                let keycode = ev.detail;

                let mut event_iter: Box<dyn Iterator<Item = Event>> = Box::new(iter::empty());

                let Some(keysym) = self.key_converter.keycode_to_keysym(keycode.into()) else {
                    trace!("unknown keycode: {keycode}");
                    break 'blk event_iter;
                };

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

                    if !modifiers_updated {
                        trace!("ignoring modifier: {keysym:?}");
                        break 'blk event_iter;
                    }

                    trace!("modifiers updated: {modifiers:?}");
                    event_iter =
                        Box::new(event_iter.chain(iter::once(Event::ModifiersChanged(*modifiers))))
                }

                let Some(key) = keysym_to_egui_key(Keysym::new(keysym.into())) else {
                    trace!("unknown keysym: {keysym:?}");
                    break 'blk event_iter;
                };

                let caps_locked = ev.state.contains(KeyButMask::LOCK);
                let modifiers = if caps_locked && is_letter_key(key) {
                    let mut modifiers_with_caps = *modifiers;
                    modifiers_with_caps.shift = !modifiers_with_caps.shift;
                    modifiers_with_caps
                } else {
                    *modifiers
                };

                trace!("key: {key:?}, pressed={pressed}, keysym={keysym:?}, keycode={keycode}");
                event_iter = Box::new(event_iter.chain(iter::once(Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false, // egui will fill this in for us!
                    modifiers,
                })));

                if emit_text_events
                    && pressed
                    && (modifiers.is_none() || modifiers == Modifiers::SHIFT)
                    && is_char_key(key, modifiers.shift)
                {
                    let column = if modifiers.shift { 1 } else { 0 };
                    if let Some(ch_keysym) = self
                        .key_converter
                        .keycode_to_keysym_column(keycode.into(), column)
                        && let Some(c) = ch_keysym.key_char()
                    {
                        trace!("text: {c:?}, keysym={ch_keysym:?}, keycode={keycode}");
                        event_iter =
                            Box::new(event_iter.chain(iter::once(Event::Text(c.to_string()))));
                    } else {
                        debug!("received unconvertable char keysym: {keysym:?}, keycode={keycode}");
                    }
                }

                event_iter
            }
            X11Event::MotionNotify(ev) => {
                let (x, y) = self.window.get_current_win_pos();
                let rel_pos = Pos2::new((ev.root_x - x) as f32, (ev.root_y - y) as f32);
                trace!(
                    "pointer moved: root=({}, {}), relative=({}, {})",
                    ev.root_x, ev.root_y, rel_pos.x, rel_pos.y
                );
                Box::new(iter::once(Event::PointerMoved(rel_pos)))
            }
            _ => Box::new(iter::empty()),
        })
    }

    pub fn handle_emit_text_events_changed(&mut self, emit_text_events: bool) -> Result<()> {
        if emit_text_events {
            self.xim_client
                .set_focus(self.xim_handler.im_id, self.xim_handler.ic_id)?;
        } else {
            self.xim_client
                .unset_focus(self.xim_handler.im_id, self.xim_handler.ic_id)?;
        }

        Ok(())
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
