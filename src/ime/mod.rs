mod ibus;
pub mod xim;
mod xim_handler;

use std::time::Duration;

use anyhow::Result;
use egui::Rect;
use log::{debug, info, warn};
use x11rb::protocol::Event as X11Event;
use x11rb::protocol::xproto::KeyPressEvent;

use crate::x11_window::X11Window;

pub const PROCESS_KEY_EVENT_TIMEOUT: Duration = Duration::from_millis(500);

pub enum ImeEvent {
    Forward(KeyPressEvent),
    Egui(egui::ImeEvent),
}

pub trait ImeBackend<'a> {
    fn handle_x11_event(&mut self, event: &X11Event, emit_text_events: bool) -> Result<bool>;
    fn take_events(&mut self) -> Vec<ImeEvent>;

    /// Ask the IME to process a raw key event. Returns true if the IME consumed it. A dead
    /// backend returns false, checked via `is_dead`.
    fn forward_key(&mut self, event: &KeyPressEvent) -> Result<bool>;

    fn is_dead(&self) -> bool;
    fn active(&self) -> bool;
    fn set_focused(&mut self, focused: bool) -> Result<()>;
    fn set_cursor_location(&mut self, rect: Rect) -> Result<()>;
    fn reset(&mut self);
    fn shutdown(&mut self);
}

/// Owns IME backend lifecycle: connect on startup, re-probe when the backend dies.
/// Prioritizes ibus over XIM (fcitx may expose an ibus-compatible interface without
/// XMODIFIERS reflecting it).
pub struct Watchers<'a> {
    window: &'a X11Window<'a>,
    ibus_watcher: ibus::IbusWatcher,
    backend: Option<Box<dyn ImeBackend<'a> + 'a>>,
}

impl<'a> Watchers<'a> {
    pub fn new(window: &'a X11Window) -> Result<Self> {
        let ibus_watcher = ibus::IbusWatcher::new();

        let mut watchers = Watchers {
            window,
            ibus_watcher,
            backend: None,
        };
        watchers.probe()?;

        if watchers.backend.is_none() {
            watchers.ibus_watcher.watch_address_dir()?;
            info!("no IME server found, waiting for ibus or XIM to appear");
        }

        Ok(watchers)
    }

    /// Try to connect to ibus first, then XIM. Returns the name of the backend that
    /// connected, if any.
    fn probe(&mut self) -> Result<Option<&'static str>> {
        match ibus::IbusBackend::new(self.window) {
            Ok(Some(backend)) => {
                info!("using ibus input method backend");
                self.backend = Some(Box::new(backend));
                return Ok(Some("ibus"));
            }
            Ok(None) => {}
            Err(e) => debug!("ibus backend init failed: {e:#}"),
        }

        match xim::XimBackend::new(self.window) {
            Ok(Some(backend)) => {
                info!("using XIM input method backend");
                self.backend = Some(Box::new(backend));
                return Ok(Some("XIM"));
            }
            Ok(None) => {}
            Err(e) => warn!("XIM backend init failed: {e:#}"),
        }

        Ok(None)
    }

    /// Called when the IME watch fd (inotify) is readable: re-probe and re-arm the watch.
    pub fn on_watch_ready(&mut self) -> Result<()> {
        if self.backend.is_some() {
            return Ok(());
        }

        self.ibus_watcher.rearm_watch()?;
        if self.probe()?.is_some() {
            info!("IME server appeared, connected");
        } else {
            self.ibus_watcher.watch_address_dir()?;
        }
        Ok(())
    }

    /// Check for backend death and re-probe. Called once per main loop iteration.
    pub fn ensure_backend(&mut self) -> Result<()> {
        let died = match &self.backend {
            Some(backend) => backend.is_dead(),
            None => false,
        };
        if died {
            warn!("IME backend died, re-probing for ibus or XIM");
            self.backend = None;
            self.ibus_watcher.watch_address_dir()?;
            self.probe()?;
            if self.backend.is_none() {
                info!("waiting for ibus or XIM to appear");
            }
        }
        Ok(())
    }

    pub fn backend(&mut self) -> Option<&mut (dyn ImeBackend<'a> + 'a)> {
        self.backend.as_deref_mut()
    }

    pub fn ibus_watcher_fd(&self) -> Option<&std::os::fd::OwnedFd> {
        self.ibus_watcher.watch_fd()
    }

    pub fn is_connected(&self) -> bool {
        self.backend.is_some()
    }

    pub fn shutdown(&mut self) {
        if let Some(backend) = self.backend.as_mut() {
            backend.shutdown();
        }
        self.backend = None;
    }
}
