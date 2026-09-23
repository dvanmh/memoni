use std::{
    iter,
    time::{Duration, Instant},
};

use crate::{
    utils::{is_char_key, is_letter_key, keysym_to_egui_key},
    x11_key_converter::X11KeyConverter,
    x11_window::X11Window,
    xim_handler::{XimEvent, XimHandler},
};
use anyhow::{Context, Result};
use egui::{
    Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, TouchPhase, Vec2,
};
use log::{debug, info, trace, warn};
use x11rb::{
    connection::{Connection as _, RequestConnection as _},
    protocol::{
        Event as X11Event,
        xfixes::{self, SelectionEvent, SelectionEventMask},
        xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask, KeyButMask, Window},
    },
    xcb_ffi::XCBConnection,
};
use xim::{Client as _, ClientError};
use xkeysym::Keysym;

const XIM_CONNECTING_TIMEOUT: Duration = Duration::from_secs(3);

type XimClient<'a> = xim::x11rb::X11rbClient<&'a XCBConnection>;

struct XimWatch {
    im_name: String,
    xim_servers_atom: u32,
    xim_server_atom: u32,
}

pub struct Input<'a> {
    pub egui_input: RawInput,
    pub xim_client: Option<XimClient<'a>>,
    pub xim_handler: XimHandler,
    window: &'a X11Window<'a>,
    key_converter: &'a X11KeyConverter<'a>,
    modifiers: Modifiers,

    xim_watch: Option<XimWatch>,
    xim_server_owner: Option<Window>,
    xim_connecting_deadline: Option<Instant>,
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

        let xim_watch = std::env::var("XMODIFIERS")
            .ok()
            .and_then(|n| n.strip_prefix("@im=").map(str::to_string))
            .map(|im_name| init_xim_watch(window, im_name))
            .transpose()?;

        let (xim_client, xim_server_owner, xim_connecting_deadline) = match &xim_watch {
            Some(watch) => {
                match try_connect_xim_server(window, watch.xim_server_atom, &watch.im_name)? {
                    Some((client, owner)) => {
                        info!("found XIM server {:?}, connecting", watch.im_name);
                        (
                            Some(client),
                            Some(owner),
                            Some(Instant::now() + XIM_CONNECTING_TIMEOUT),
                        )
                    }
                    None => {
                        info!(
                            "no XIM server yet, waiting for {:?} to appear",
                            watch.im_name
                        );
                        (None, None, None)
                    }
                }
            }
            None => {
                info!("XMODIFIERS not set, XIM disabled");
                (None, None, None)
            }
        };

        Ok(Input {
            egui_input,
            xim_client,
            xim_handler: XimHandler::new(window.win_id.get()),
            window,
            key_converter,
            modifiers: Modifiers::default(),

            xim_watch,
            xim_server_owner,
            xim_connecting_deadline,
        })
    }

    pub fn handle_event(&mut self, event: &X11Event, emit_text_events: bool) -> Result<()> {
        if self.handle_xim_lifecycle(event)? {
            return Ok(());
        }

        let handled_by_xim = match self.xim_client.as_mut() {
            Some(xim_client) => xim_client.filter_event(event, &mut self.xim_handler)?,
            None => false,
        };
        if handled_by_xim {
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

            // A new connection to XIM server is fully established by this event
            if self.xim_handler.connected && self.xim_connecting_deadline.take().is_some() {
                if let Some(watch) = &self.xim_watch {
                    info!("connected to XIM server {:?}", watch.im_name);
                }

                // Search mode may have been entered while disconnected, in which case set_focus was never sent
                if emit_text_events && let Some(xim_client) = self.xim_client.as_mut() {
                    xim_client.set_focus(self.xim_handler.im_id, self.xim_handler.ic_id)?;
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
                let pressed = matches!(event, X11Event::KeyPress(_));
                let keycode = ev.detail;
                let Some(keysym) = self.key_converter.keycode_to_keysym(keycode.into()) else {
                    trace!("unknown keycode: {keycode}");
                    break 'blk Box::new(iter::empty());
                };

                let mut next_modifiers = *modifiers;
                if keysym.is_modifier_key() {
                    if keysym == Keysym::Alt_L || keysym == Keysym::Alt_R {
                        next_modifiers.alt = pressed;
                    }
                    if keysym == Keysym::Control_L || keysym == Keysym::Control_R {
                        next_modifiers.ctrl = pressed;
                    }
                    if keysym == Keysym::Shift_L || keysym == Keysym::Shift_R {
                        next_modifiers.shift = pressed;
                    }
                    if keysym == Keysym::Super_L || keysym == Keysym::Super_R {
                        // egui has no Super slot on Linux, so repurpose the unused-on-Linux mac_cmd bit for it
                        next_modifiers.mac_cmd = pressed;
                    }
                }

                // XIM servers don't forward back Super keymaps
                if !next_modifiers.mac_cmd
                    && forward_xim
                    && let Some(xim_client) = self.xim_client.as_mut()
                    && self.xim_handler.connected
                {
                    trace!("forwarding key event to XIM server: {ev:?}");
                    xim_client.forward_event(
                        self.xim_handler.im_id,
                        self.xim_handler.ic_id,
                        xim::ForwardEventFlag::empty(),
                        ev,
                    )?;
                    break 'blk Box::new(iter::empty());
                }

                let mut event_iter: Box<dyn Iterator<Item = Event>> = Box::new(iter::empty());

                if keysym.is_modifier_key() {
                    if next_modifiers == *modifiers {
                        debug!("ignoring modifier: {keysym:?}");
                        break 'blk event_iter;
                    }

                    *modifiers = next_modifiers;
                    trace!("modifiers updated: {modifiers:?}");
                    event_iter =
                        Box::new(event_iter.chain(iter::once(Event::ModifiersChanged(*modifiers))));
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

    fn handle_xim_lifecycle(&mut self, event: &X11Event) -> Result<bool> {
        let Some(watch) = self.xim_watch.as_ref() else {
            return Ok(false);
        };

        let xim_servers_atom = watch.xim_servers_atom;
        let xim_server_atom = watch.xim_server_atom;
        match event {
            X11Event::XfixesSelectionNotify(ev) if ev.selection == xim_server_atom => {
                match ev.subtype {
                    SelectionEvent::SET_SELECTION_OWNER => {
                        if self.xim_client.is_some() {
                            if ev.owner == x11rb::NONE || Some(ev.owner) != self.xim_server_owner {
                                warn!("XIM server selection owner changed, disconnecting");
                                self.teardown_xim();
                            }
                        } else if ev.owner != x11rb::NONE
                            && let Some((client, owner)) = try_connect_xim_server(
                                self.window,
                                xim_server_atom,
                                &watch.im_name,
                            )?
                        {
                            self.connect_xim(client, owner);
                        }
                    }
                    SelectionEvent::SELECTION_WINDOW_DESTROY
                    | SelectionEvent::SELECTION_CLIENT_CLOSE
                        if self.xim_client.is_some() =>
                    {
                        warn!("XIM server died ({:?}), disconnecting", ev.subtype);
                        self.teardown_xim();
                    }
                    _ => {}
                }
                Ok(true)
            }

            X11Event::PropertyNotify(ev)
                if ev.atom == xim_servers_atom && self.xim_client.is_none() =>
            {
                if let Some((client, owner)) =
                    try_connect_xim_server(self.window, xim_server_atom, &watch.im_name)?
                {
                    self.connect_xim(client, owner);
                }
                Ok(true)
            }

            _ => {
                if self.xim_client.is_some()
                    && !self.xim_handler.connected
                    && let Some(deadline) = self.xim_connecting_deadline
                    && Instant::now() >= deadline
                {
                    warn!("XIM connecting timed out, disconnecting");
                    self.teardown_xim();
                }
                Ok(false)
            }
        }
    }

    pub fn handle_emit_text_events_changed(&mut self, emit_text_events: bool) -> Result<()> {
        let Some(xim_client) = self.xim_client.as_mut() else {
            return Ok(());
        };

        if emit_text_events {
            xim_client.set_focus(self.xim_handler.im_id, self.xim_handler.ic_id)?;
        } else {
            xim_client.unset_focus(self.xim_handler.im_id, self.xim_handler.ic_id)?;
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

    fn connect_xim(&mut self, client: XimClient<'a>, owner: Window) {
        if let Some(watch) = &self.xim_watch {
            info!("XIM server {:?} appeared, connecting", watch.im_name);
        }

        self.xim_client = Some(client);
        self.xim_server_owner = Some(owner);
        self.xim_connecting_deadline = Some(Instant::now() + XIM_CONNECTING_TIMEOUT);
    }

    fn teardown_xim(&mut self) {
        self.xim_client = None;
        self.xim_server_owner = None;
        self.xim_connecting_deadline = None;
        self.xim_handler = XimHandler::new(self.window.win_id.get());

        if let Some(watch) = &self.xim_watch {
            info!("waiting for XIM server {:?} to reappear", watch.im_name);
        }
    }
}

fn init_xim_watch(window: &X11Window, im_name: String) -> Result<XimWatch> {
    let conn = &window.conn;

    let xim_servers_atom = conn.intern_atom(false, b"XIM_SERVERS")?.reply()?.atom;
    let xim_server_atom = conn
        .intern_atom(false, format!("@server={im_name}").as_bytes())?
        .reply()?
        .atom;

    debug!("ensuring XFixes exists");
    conn.prefetch_extension_information(xfixes::X11_EXTENSION_NAME)?;
    conn.extension_information(xfixes::X11_EXTENSION_NAME)?
        .context("XFixes not found")?;
    xfixes::query_version(conn, 5, 0)?.reply()?;

    let event_mask = SelectionEventMask::SET_SELECTION_OWNER
        | SelectionEventMask::SELECTION_WINDOW_DESTROY
        | SelectionEventMask::SELECTION_CLIENT_CLOSE;
    xfixes::select_selection_input(conn, window.win_id.get(), xim_server_atom, event_mask)?;

    conn.change_window_attributes(
        window.screen.root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    conn.flush()?;

    Ok(XimWatch {
        im_name,
        xim_servers_atom,
        xim_server_atom,
    })
}

fn try_connect_xim_server<'a>(
    window: &'a X11Window,
    xim_server_atom: u32,
    im_name: &str,
) -> Result<Option<(XimClient<'a>, Window)>> {
    let owner = window
        .conn
        .get_selection_owner(xim_server_atom)?
        .reply()?
        .owner;
    if owner == x11rb::NONE {
        return Ok(None);
    }

    match xim::x11rb::X11rbClient::init(&window.conn, window.screen_num, Some(im_name)) {
        Ok(client) => Ok(Some((client, owner))),
        Err(ClientError::NoXimServer | ClientError::InvalidReply) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
