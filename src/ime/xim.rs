use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{debug, info, warn};
use x11rb::{
    connection::{Connection as _, RequestConnection as _},
    protocol::{
        Event as X11Event,
        xfixes::{self, SelectionEvent, SelectionEventMask},
        xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask, KeyPressEvent, Window},
    },
    xcb_ffi::XCBConnection,
};
use xim::{Client as _, ClientError};

use super::{ImeBackend, ImeEvent};
use crate::{
    ime::xim_handler::{XimEvent, XimHandler},
    x11_window::X11Window,
};

const XIM_CONNECTING_TIMEOUT: Duration = Duration::from_secs(3);

type XimClient<'a> = xim::x11rb::X11rbClient<&'a XCBConnection>;

struct XimWatch {
    im_name: String,
    xim_servers_atom: u32,
    xim_server_atom: u32,
}

pub struct XimBackend<'a> {
    client: Option<XimClient<'a>>,
    handler: XimHandler,
    window: &'a X11Window<'a>,
    watch: Option<XimWatch>,
    xim_server_owner: Option<Window>,
    xim_connecting_deadline: Option<Instant>,
}

impl<'a> XimBackend<'a> {
    pub fn new(window: &'a X11Window) -> Result<Option<Self>> {
        let im_name = match std::env::var("XMODIFIERS")
            .ok()
            .and_then(|n| n.strip_prefix("@im=").map(str::to_string))
        {
            Some(name) => name,
            None => {
                info!("XMODIFIERS not set, XIM disabled");
                return Ok(None);
            }
        };

        let watch = init_xim_watch(window, im_name)?;

        let (client, xim_server_owner, xim_connecting_deadline) =
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
            };

        Ok(Some(XimBackend {
            client,
            handler: XimHandler::new(window.win_id.get()),
            window,
            watch: Some(watch),
            xim_server_owner,
            xim_connecting_deadline,
        }))
    }

    pub fn connected(&self) -> bool {
        self.client.is_some() && self.handler.connected
    }

    fn connect_xim(&mut self, client: XimClient<'a>, owner: Window) {
        if let Some(watch) = &self.watch {
            info!("XIM server {:?} appeared, connecting", watch.im_name);
        }

        self.client = Some(client);
        self.xim_server_owner = Some(owner);
        self.xim_connecting_deadline = Some(Instant::now() + XIM_CONNECTING_TIMEOUT);
    }

    fn teardown_xim(&mut self) {
        self.client = None;
        self.xim_server_owner = None;
        self.xim_connecting_deadline = None;
        self.handler = XimHandler::new(self.window.win_id.get());

        if let Some(watch) = &self.watch {
            info!("waiting for XIM server {:?} to reappear", watch.im_name);
        }
    }

    fn handle_xim_lifecycle(&mut self, event: &X11Event) -> Result<bool> {
        let Some(watch) = self.watch.as_ref() else {
            return Ok(false);
        };

        let xim_servers_atom = watch.xim_servers_atom;
        let xim_server_atom = watch.xim_server_atom;
        match event {
            X11Event::XfixesSelectionNotify(ev) if ev.selection == xim_server_atom => {
                match ev.subtype {
                    SelectionEvent::SET_SELECTION_OWNER => {
                        if self.client.is_some() {
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
                        if self.client.is_some() =>
                    {
                        warn!("XIM server died ({:?}), disconnecting", ev.subtype);
                        self.teardown_xim();
                    }
                    _ => {}
                }
                Ok(true)
            }

            X11Event::PropertyNotify(ev)
                if ev.atom == xim_servers_atom && self.client.is_none() =>
            {
                if let Some((client, owner)) =
                    try_connect_xim_server(self.window, xim_server_atom, &watch.im_name)?
                {
                    self.connect_xim(client, owner);
                }
                Ok(true)
            }

            _ => {
                if self.client.is_some()
                    && !self.handler.connected
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
}

impl<'a> ImeBackend<'a> for XimBackend<'a> {
    fn handle_x11_event(&mut self, event: &X11Event, emit_text_events: bool) -> Result<bool> {
        if self.handle_xim_lifecycle(event)? {
            return Ok(true);
        }

        let handled_by_xim = match self.client.as_mut() {
            Some(xim_client) => xim_client.filter_event(event, &mut self.handler)?,
            None => false,
        };

        if handled_by_xim
            && self.handler.connected
            && self.xim_connecting_deadline.take().is_some()
            && let Some(watch) = &self.watch
        {
            info!("connected to XIM server {:?}", watch.im_name);

            // Search mode may have been entered while disconnected, in which case set_focus was
            // never sent
            if emit_text_events && let Some(xim_client) = self.client.as_mut() {
                xim_client.set_focus(self.handler.im_id, self.handler.ic_id)?;
            }
        }

        Ok(handled_by_xim)
    }

    fn forward_key(&mut self, event: &KeyPressEvent) -> Result<bool> {
        let Some(xim_client) = self.client.as_mut() else {
            return Ok(false);
        };
        if !self.handler.connected {
            return Ok(false);
        }

        xim_client.forward_event(
            self.handler.im_id,
            self.handler.ic_id,
            xim::ForwardEventFlag::empty(),
            event,
        )?;
        Ok(true)
    }

    fn take_events(&mut self) -> Vec<ImeEvent> {
        std::mem::take(&mut self.handler.events)
            .into_iter()
            .map(|xim_event| match xim_event {
                XimEvent::Forward(key_press_event) => ImeEvent::Forward(key_press_event),
                XimEvent::Egui(ime_event) => ImeEvent::Egui(ime_event),
            })
            .collect()
    }

    fn is_dead(&self) -> bool {
        false
    }

    fn active(&self) -> bool {
        self.connected()
    }

    fn set_focused(&mut self, focused: bool) -> Result<()> {
        let Some(xim_client) = self.client.as_mut() else {
            return Ok(());
        };

        if focused {
            xim_client.set_focus(self.handler.im_id, self.handler.ic_id)?;
        } else {
            xim_client.unset_focus(self.handler.im_id, self.handler.ic_id)?;
        }

        Ok(())
    }

    fn set_cursor_location(&mut self, _rect: egui::Rect) -> Result<()> {
        Ok(())
    }

    fn reset(&mut self) {}

    fn shutdown(&mut self) {}
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
