use std::{
    os::fd::OwnedFd,
    sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel},
};

use anyhow::{Result, bail};
use egui::Rect;
use log::{debug, info, trace, warn};
use x11rb::protocol::xproto::KeyPressEvent;
use x11rb::protocol::{Event as X11Event, xproto::KeyButMask};
use zbus::blocking::connection::Builder as BlockingConnectionBuilder;
use zbus::blocking::{Connection, Proxy, fdo::DBusProxy};
use zbus::zvariant::{ObjectPath, Value};

use super::{ImeBackend, ImeEvent, PROCESS_KEY_EVENT_TIMEOUT};
use crate::x11_window::X11Window;

const IBUS_BUS_NAME: &str = "org.freedesktop.IBus";
const IBUS_OBJECT_PATH: &str = "/org/freedesktop/IBus";

const IBUS_RELEASE_MASK: u32 = 1 << 30;
const IBUS_SHIFT_MASK: u32 = 1 << 0;
const IBUS_CONTROL_MASK: u32 = 1 << 2;
const IBUS_MOD1_MASK: u32 = 1 << 3;

const XKB_SHIFT_MASK: u16 = 1 << 0;
const XKB_CONTROL_MASK: u16 = 1 << 2;
const XKB_MOD1_MASK: u16 = 1 << 3;

const SIGNAL_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug)]
enum IbusSignal {
    Commit(String),
    Preedit { text: String, visible: bool },
    ForwardKeyEvent { keycode: u32, state: u32 },
    DeleteSurrounding { offset: i32, nchars: u32 },
}

pub struct IbusBackend {
    connection: Connection,
    context_path: ObjectPath<'static>,
    signal_receiver: Receiver<IbusSignal>,
    signal_thread: Option<std::thread::JoinHandle<()>>,
    dead: bool,
}

impl IbusBackend {
    pub fn new(_window: &X11Window) -> Result<Option<Self>> {
        let Some(connection) = connect_to_ibus_bus() else {
            return Ok(None);
        };

        let bus_proxy = DBusProxy::new(&connection)?;
        match bus_proxy.name_has_owner(IBUS_BUS_NAME.try_into()?)? {
            true => {}
            false => {
                info!("ibus daemon not found on the bus, skipping ibus backend");
                return Ok(None);
            }
        }

        let ibus_proxy = Proxy::new(
            &connection,
            IBUS_BUS_NAME,
            IBUS_OBJECT_PATH,
            "org.freedesktop.IBus",
        )?;
        let body = (env!("CARGO_PKG_NAME"),);
        let reply = ibus_proxy.call_method("CreateInputContext", &body)?;
        let body = reply.body();
        let (context_path,): (ObjectPath,) = body.deserialize()?;

        let context_path = context_path.to_owned();
        info!("created ibus input context at {context_path}");

        let (signal_sender, signal_receiver) = sync_channel(SIGNAL_CHANNEL_CAPACITY);
        let thread_connection = connection.clone();
        let thread_path = context_path.clone();
        let signal_thread = std::thread::Builder::new()
            .name("memoni-ibus-signals".into())
            .spawn(move || {
                run_signal_listener(thread_connection, thread_path, signal_sender);
            })?;

        Ok(Some(IbusBackend {
            connection,
            context_path,
            signal_receiver,
            signal_thread: Some(signal_thread),
            dead: false,
        }))
    }

    fn context_proxy(&self) -> Result<Proxy<'_>> {
        Ok(Proxy::new(
            &self.connection,
            IBUS_BUS_NAME,
            self.context_path.as_str(),
            "org.freedesktop.IBus.InputContext",
        )?)
    }

    fn mark_dead(&mut self) {
        if !self.dead {
            warn!("ibus backend connection lost, marking as dead");
            self.dead = true;
        }
    }
}

fn connect_to_ibus_bus() -> Option<Connection> {
    if let Ok(session_address) = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        && let Ok(connection) = connect_with_address(&session_address)
    {
        return Some(connection);
    }

    let address_file = ibus_address_file()?;
    let contents = std::fs::read_to_string(&address_file).ok()?;
    let ibus_address = contents
        .lines()
        .find_map(|line| line.strip_prefix("IBUS_ADDRESS="))?
        .to_owned();
    debug!("connecting to ibus bus at {ibus_address:?}");
    connect_with_address(&ibus_address).ok()
}

fn connect_with_address(address: &str) -> Result<Connection> {
    Ok(BlockingConnectionBuilder::address(address)?
        .method_timeout(PROCESS_KEY_EVENT_TIMEOUT)
        .build()?)
}

fn ibus_address_file() -> Option<std::path::PathBuf> {
    let bus_dir = dirs::home_dir()?.join(".config/ibus/bus");

    // ibus uses the dbus machine id in the file name; it may live in either location and the
    // /etc one may differ from /var/lib, so try both.
    let mut machine_ids = Vec::new();
    for path in ["/var/lib/dbus/machine-id", "/etc/machine-id"] {
        if let Ok(id) = std::fs::read_to_string(path) {
            let id = id.trim().to_owned();
            if !id.is_empty() {
                machine_ids.push(id);
            }
        }
    }

    let display = std::env::var("DISPLAY").ok()?;
    let display = display
        .strip_prefix(':')
        .unwrap_or(&display)
        .split('.')
        .next()?
        .to_owned();

    let candidates = machine_ids
        .iter()
        .map(|id| bus_dir.join(format!("{id}-unix-{display}")))
        .chain(std::iter::once(
            bus_dir.join(format!("machine-id-unix-{display}")),
        ));
    candidates
        .into_iter()
        .find(|path| path.exists())
        .or_else(|| {
            // Fall back to the newest address file for this display
            let suffix = format!("-unix-{display}");
            let mut newest: Option<std::path::PathBuf> = None;
            let mut newest_mtime = None;
            if let Ok(entries) = std::fs::read_dir(&bus_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.ends_with(&suffix)
                        && let Ok(metadata) = entry.metadata()
                        && let Ok(modified) = metadata.modified()
                        && newest_mtime.is_none_or(|current| modified > current)
                    {
                        newest_mtime = Some(modified);
                        newest = Some(entry.path());
                    }
                }
            }
            newest
        })
}

fn run_signal_listener(
    connection: Connection,
    context_path: ObjectPath<'static>,
    sender: SyncSender<IbusSignal>,
) {
    let result = (|| -> Result<()> {
        let proxy = Proxy::new(
            &connection,
            IBUS_BUS_NAME,
            context_path.as_str(),
            "org.freedesktop.IBus.InputContext",
        )?;
        let signals = proxy.receive_all_signals()?;
        for message in signals {
            let member = message.header().member().map(|m| m.to_owned());
            let Some(member) = member else {
                continue;
            };
            let signal = match member.as_str() {
                "CommitText" => {
                    let body = message.body();
                    let (text,): (Value,) = body.deserialize()?;
                    IbusSignal::Commit(value_into_string(text)?)
                }
                "UpdatePreeditTextWithMode" => {
                    let body = message.body();
                    let (text, _cursor, _pos, visible, _mode): (Value, u32, u32, bool, u32) =
                        body.deserialize()?;
                    IbusSignal::Preedit {
                        text: value_into_string(text)?,
                        visible,
                    }
                }
                "UpdatePreeditText" => {
                    let body = message.body();
                    let (text, _cursor, _pos, visible): (Value, u32, u32, bool) =
                        body.deserialize()?;
                    IbusSignal::Preedit {
                        text: value_into_string(text)?,
                        visible,
                    }
                }
                "ForwardKeyEvent" => {
                    let body = message.body();
                    let (keyval, keycode, state): (u32, u32, u32) = body.deserialize()?;
                    let _ = keyval;
                    IbusSignal::ForwardKeyEvent { keycode, state }
                }
                "DeleteSurroundingText" => {
                    let body = message.body();
                    let (offset, nchars): (i32, u32) = body.deserialize()?;
                    IbusSignal::DeleteSurrounding { offset, nchars }
                }
                _ => continue,
            };
            if sender.send(signal).is_err() {
                break;
            }
        }
        Ok(())
    })();

    if let Err(e) = result {
        warn!("ibus signal listener failed: {e:#}");
    }
    info!("ibus signal listener exiting");
}

fn value_into_string(value: Value) -> Result<String> {
    match value {
        Value::Str(s) => Ok(s.as_str().to_owned()),
        Value::Value(inner) => value_into_string(*inner),
        other => bail!("unexpected value type for text: {other:?}"),
    }
}

fn x_state_to_ibus_state(state: u16) -> u32 {
    let mut ibus_state = 0;
    if state & XKB_SHIFT_MASK != 0 {
        ibus_state |= IBUS_SHIFT_MASK;
    }
    if state & XKB_CONTROL_MASK != 0 {
        ibus_state |= IBUS_CONTROL_MASK;
    }
    if state & XKB_MOD1_MASK != 0 {
        ibus_state |= IBUS_MOD1_MASK;
    }
    ibus_state
}

impl<'a> ImeBackend<'a> for IbusBackend {
    fn handle_x11_event(&mut self, _event: &X11Event, _emit_text_events: bool) -> Result<bool> {
        Ok(false)
    }

    fn take_events(&mut self) -> Vec<ImeEvent> {
        let mut events = Vec::new();
        loop {
            match self.signal_receiver.try_recv() {
                Ok(IbusSignal::Commit(text)) => {
                    debug!("ibus commit: {text:?}");
                    events.push(ImeEvent::Egui(egui::ImeEvent::Commit(text)));
                }
                Ok(IbusSignal::Preedit { text, visible }) => {
                    let active_range_chars = if visible && !text.is_empty() {
                        Some(0..text.chars().count())
                    } else {
                        None
                    };
                    debug!("ibus preedit: {text:?}, visible={visible}");
                    events.push(ImeEvent::Egui(egui::ImeEvent::Preedit {
                        text,
                        active_range_chars,
                    }));
                }
                Ok(IbusSignal::ForwardKeyEvent { keycode, state }) => {
                    debug!("ibus forward: keycode={keycode} state={state:#x}");
                    let key_event = synthesize_key_event(keycode, state);
                    events.push(ImeEvent::Forward(key_event));
                }
                Ok(IbusSignal::DeleteSurrounding { offset, nchars }) => {
                    debug!("ibus delete surrounding: offset={offset} nchars={nchars}");
                    let (before, after) = if offset <= 0 {
                        (
                            offset.unsigned_abs() as usize,
                            (nchars as i64 + offset as i64).max(0) as usize,
                        )
                    } else {
                        (0usize, nchars as usize)
                    };
                    events.push(ImeEvent::Egui(egui::ImeEvent::DeleteSurrounding {
                        before_chars: before,
                        after_chars: after,
                    }));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.mark_dead();
                    break;
                }
            }
        }
        events
    }

    fn forward_key(&mut self, event: &KeyPressEvent) -> Result<bool> {
        if self.dead {
            return Ok(false);
        }

        let keycode = u32::from(event.detail.saturating_sub(8));
        let state = x_state_to_ibus_state(u16::from(event.state));
        trace!("ProcessKeyEvent keycode={keycode} state={state:#x}");

        let proxy = self.context_proxy()?;
        let result = proxy.call::<_, _, bool>("ProcessKeyEvent", &(0u32, keycode, state));
        drop(proxy);
        match result {
            Ok(handled) => {
                trace!("ProcessKeyEvent -> {handled}");
                Ok(handled)
            }
            Err(e) => {
                warn!("ProcessKeyEvent failed: {e:#}");
                self.mark_dead();
                Ok(false)
            }
        }
    }

    fn is_dead(&self) -> bool {
        self.dead
    }

    fn active(&self) -> bool {
        !self.dead
    }

    fn set_focused(&mut self, focused: bool) -> Result<()> {
        if self.dead {
            return Ok(());
        }

        let method = if focused { "FocusIn" } else { "FocusOut" };
        let result = {
            let proxy = self.context_proxy()?;
            proxy.call_noreply(method, &())
        };
        if let Err(e) = result {
            warn!("ibus {method} failed: {e:#}");
            if focused {
                self.mark_dead();
            }
        }
        Ok(())
    }

    fn set_cursor_location(&mut self, rect: Rect) -> Result<()> {
        if self.dead {
            return Ok(());
        }

        let proxy = self.context_proxy()?;
        if let Err(e) = proxy.call_noreply(
            "SetCursorLocationRelative",
            &(
                rect.left() as i32,
                rect.top() as i32,
                rect.width() as i32,
                rect.height() as i32,
            ),
        ) {
            trace!("SetCursorLocationRelative failed: {e:#}");
        }
        Ok(())
    }

    fn reset(&mut self) {
        if self.dead {
            return;
        }

        match self.context_proxy() {
            Ok(proxy) => {
                if let Err(e) = proxy.call_noreply("Reset", &()) {
                    debug!("ibus Reset failed: {e:#}");
                }
            }
            Err(e) => debug!("ibus Reset failed: {e:#}"),
        }
    }

    fn shutdown(&mut self) {
        if self.dead {
            return;
        }

        if let Ok(proxy) = self.context_proxy()
            && let Err(e) = proxy.call_noreply("FocusOut", &())
        {
            debug!("ibus FocusOut on shutdown failed: {e:#}");
        }
        if let Some(signal_thread) = self.signal_thread.take() {
            signal_thread.join().ok();
        }
    }
}

fn synthesize_key_event(keycode: u32, state: u32) -> KeyPressEvent {
    let detail = (keycode + 8).min(255) as u8;
    let mut x_state: u16 = 0;
    if state & IBUS_SHIFT_MASK != 0 {
        x_state |= XKB_SHIFT_MASK;
    }
    if state & IBUS_CONTROL_MASK != 0 {
        x_state |= XKB_CONTROL_MASK;
    }
    if state & IBUS_MOD1_MASK != 0 {
        x_state |= XKB_MOD1_MASK;
    }

    let response_type = if state & IBUS_RELEASE_MASK != 0 {
        x11rb::protocol::xproto::KEY_RELEASE_EVENT
    } else {
        x11rb::protocol::xproto::KEY_PRESS_EVENT
    };

    KeyPressEvent {
        response_type,
        detail,
        sequence: 0,
        time: 0,
        root: 0,
        event: 0,
        child: 0,
        root_x: 0,
        root_y: 0,
        event_x: 0,
        event_y: 0,
        // xproto's From<u16> impl for KeyButMask
        state: KeyButMask::from(x_state),
        same_screen: true,
    }
}

/// Watches ~/.config/ibus/bus with inotify so the main loop can re-probe (timer-less)
/// when ibus writes a fresh address file after starting.
pub struct IbusWatcher {
    inotify_fd: Option<OwnedFd>,
}

impl IbusWatcher {
    pub fn new() -> Self {
        Self { inotify_fd: None }
    }

    pub fn watch_address_dir(&mut self) -> Result<()> {
        self.inotify_fd = None;

        let Some(bus_dir) = ibus_bus_dir() else {
            return Ok(());
        };
        if !bus_dir.exists() {
            debug!("ibus bus dir {} does not exist yet", bus_dir.display());
            return Ok(());
        }

        let fd = rustix::fs::inotify::init(rustix::fs::inotify::CreateFlags::CLOEXEC)?;
        rustix::fs::inotify::add_watch(
            &fd,
            &bus_dir,
            rustix::fs::inotify::WatchFlags::CREATE
                | rustix::fs::inotify::WatchFlags::CLOSE_WRITE
                | rustix::fs::inotify::WatchFlags::MOVED_TO
                | rustix::fs::inotify::WatchFlags::DELETE,
        )?;
        debug!(
            "watching {} for ibus address file changes",
            bus_dir.display()
        );
        self.inotify_fd = Some(fd);
        Ok(())
    }

    pub fn rearm_watch(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn watch_fd(&self) -> Option<&OwnedFd> {
        self.inotify_fd.as_ref()
    }
}

fn ibus_bus_dir() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".config/ibus/bus"))
}
