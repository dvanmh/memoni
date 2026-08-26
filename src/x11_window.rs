extern crate x11rb;

use std::os::unix::ffi::OsStrExt as _;
use std::{cell::Cell, iter::once};

use anyhow::{Result, anyhow, bail};
use log::{debug, info, trace, warn};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, *};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{connection::Connection, protocol::randr};

use crate::config::{Config, Dimensions, LayoutConfig, WindowPositionMode};
use crate::selection::SelectionType;
use crate::timerfd_source::TimerfdSource;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_CLIENT_MACHINE,
        WM_STATE,
        UTF8_STRING,
        _NET_CURRENT_DESKTOP,
        _NET_DESKTOP_VIEWPORT,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
    }
}

#[derive(Debug)]
struct Viewport {
    x: u32,
    y: u32,
}

const MAX_GRAB_RETRIES: u8 = 255;

pub struct X11Window<'a> {
    pub conn: XCBConnection,
    pub screen: Screen,
    pub screen_num: usize,
    pub atoms: Atoms,
    pub win_id: Cell<u32>,
    pub selection_type: SelectionType,
    pub dimensions: Dimensions,
    pub win_opened_pointer: Cell<(i16, i16, Option<Window>)>,
    config: &'a Config,
    shown_win_event_mask: EventMask,
    hidden_win_event_mask: EventMask,
    win_pos: Cell<(i16, i16)>,
    win_placed_above_pointer: Cell<bool>,
    keyboard_grab_retry_count: Cell<u8>,
    pointer_grab_retry_count: Cell<u8>,
}

impl<'a> X11Window<'a> {
    pub fn new(config: &'a Config, selection_type: SelectionType) -> Result<Self> {
        info!("connecting to X11");
        let (conn, screen_num) = XCBConnection::connect(None)?;
        let setup = conn.setup();
        let screen = setup.roots[screen_num].to_owned();
        let atoms = Atoms::new(&conn)?.reply()?;

        let win_id = conn.generate_id()?;
        let shown_win_event_mask = EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION;
        let hidden_win_event_mask = EventMask::STRUCTURE_NOTIFY;

        let x11_window = X11Window {
            conn,
            screen,
            screen_num,
            atoms,
            win_id: Cell::new(win_id),
            selection_type,
            dimensions: config.layout.window_dimensions,
            config,
            shown_win_event_mask,
            hidden_win_event_mask,
            win_pos: Cell::new((0, 0)),
            win_opened_pointer: Cell::new((0, 0, None)),
            win_placed_above_pointer: Cell::new(false),
            keyboard_grab_retry_count: Cell::new(0),
            pointer_grab_retry_count: Cell::new(0),
        };

        info!("creating main window with id {win_id}");
        x11_window.create_main_window()?;

        Ok(x11_window)
    }

    pub fn recreate_main_window(&self) -> Result<()> {
        let win_id = self.conn.generate_id()?;
        info!("recreating main window with id {win_id}");

        self.win_id.set(win_id);
        self.create_main_window()?;

        Ok(())
    }

    fn create_main_window(&self) -> Result<()> {
        let Self {
            conn,
            screen,
            win_id,
            hidden_win_event_mask,
            config,
            selection_type,
            atoms,
            ..
        } = self;
        let win_id = win_id.get();

        let target_depth = 32;
        let target_visual_id = screen
            .allowed_depths
            .iter()
            .find(|d| d.depth == target_depth)
            .and_then(|d| {
                d.visuals
                    .iter()
                    .find(|v| v.class == VisualClass::TRUE_COLOR)
            })
            .map(|v| v.visual_id);

        let colormap = if let Some(visual_id) = target_visual_id {
            let colormap_id = conn.generate_id()?;
            conn.create_colormap(ColormapAlloc::NONE, colormap_id, screen.root, visual_id)?
                .check()?;
            Some(colormap_id)
        } else {
            None
        };

        let win_aux = CreateWindowAux::new()
            .event_mask(*hidden_win_event_mask)
            .background_pixel(*config.theme.background)
            .win_gravity(Gravity::NORTH_WEST)
            .colormap(colormap)
            .border_pixel(0)
            .override_redirect(1);
        conn.create_window(
            target_visual_id
                .map(|_| target_depth)
                .unwrap_or(screen.root_depth),
            win_id,
            screen.root,
            0,
            0,
            config.layout.window_dimensions.width,
            config.layout.window_dimensions.height,
            0,
            WindowClass::INPUT_OUTPUT,
            target_visual_id.unwrap_or(0),
            &win_aux,
        )?
        .check()?;

        let wm_name = format!("Memoni - {}", selection_type).into_bytes();
        conn.change_property8(
            PropMode::REPLACE,
            win_id,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            &wm_name,
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            win_id,
            atoms._NET_WM_NAME,
            atoms.UTF8_STRING,
            &wm_name,
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            win_id,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            &format!(
                "memoni-{}\0Memoni\0",
                selection_type.to_string().to_lowercase()
            )
            .into_bytes(),
        )?
        .check()?;

        conn.change_property32(
            PropMode::REPLACE,
            win_id,
            atoms._NET_WM_STATE,
            AtomEnum::ATOM,
            &[atoms._NET_WM_STATE_ABOVE],
        )?
        .check()?;
        conn.change_property32(
            PropMode::REPLACE,
            win_id,
            atoms._NET_WM_PID,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            win_id,
            atoms.WM_CLIENT_MACHINE,
            AtomEnum::STRING,
            gethostname::gethostname().as_bytes(),
        )?
        .check()?;
        conn.flush()?;

        Ok(())
    }

    pub fn update_window_pos(&self) -> Result<()> {
        info!("updating window position");
        let pointer = self.conn.query_pointer(self.screen.root)?.reply()?;
        let pointer_window =
            find_window_under_pointer(&self.conn, pointer.child, self.atoms.WM_STATE)?;

        self.win_opened_pointer
            .set((pointer.root_x, pointer.root_y, pointer_window));

        let (x, y, placed_above_pointer) = self.calculate_window_pos()?;
        self.conn.configure_window(
            self.win_id.get(),
            &ConfigureWindowAux::new().x(x as i32).y(y as i32),
        )?;
        self.win_pos.set((x, y));
        self.win_placed_above_pointer.set(placed_above_pointer);
        info!(
            "window position updated: ({x}, {y}), {} the pointer",
            if placed_above_pointer {
                "above"
            } else {
                "below"
            }
        );

        Ok(())
    }

    pub fn show_window(&self) -> Result<()> {
        debug!("mapping window");
        self.conn.configure_window(
            self.win_id.get(),
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        self.conn.map_window(self.win_id.get())?;
        Ok(())
    }

    pub fn hide_window(&self) -> Result<()> {
        debug!("unmapping window");
        self.conn.unmap_window(self.win_id.get())?;
        Ok(())
    }

    // Have to retry because if memoni is triggered from a window manager (e.g. i3)
    // keymap, the WM is probably still grabbing the keyboard and not ungrabbing immediately
    pub fn grab_keyboard(&self, timer: &TimerfdSource) -> Result<()> {
        let retry_count = self.keyboard_grab_retry_count.get();
        if retry_count == 0 {
            debug!("grabbing keyboard");
        } else {
            trace!("retrying to grab keyboard: retry {retry_count}");
        }

        let grab_keyboard = self.conn.grab_keyboard(
            true,
            self.screen.root,
            x11rb::CURRENT_TIME,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;

        if grab_keyboard.reply()?.status == GrabStatus::SUCCESS {
            debug!("keyboard grabbed successfully");
            self.keyboard_grab_retry_count.set(0);
        } else {
            let new_count = self.keyboard_grab_retry_count.get() + 1;
            if new_count < MAX_GRAB_RETRIES {
                self.keyboard_grab_retry_count.set(new_count);
                timer.set_timer(10)?;
            } else {
                warn!("failed to grab keyboard after {MAX_GRAB_RETRIES} retries");
                self.keyboard_grab_retry_count.set(0);
            }
        }
        Ok(())
    }

    // Same reason to retry as above, just applied to pointer keymaps
    pub fn grab_pointer(&self, timer: &TimerfdSource) -> Result<()> {
        let retry_count = self.pointer_grab_retry_count.get();
        if retry_count == 0 {
            debug!("grabbing pointer");
        } else {
            trace!("retrying to grab pointer: retry {retry_count}");
        }

        let grab_pointer = self.conn.grab_pointer(
            true,
            self.screen.root,
            EventMask::BUTTON_RELEASE | EventMask::BUTTON_MOTION | EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            self.screen.root,
            x11rb::NONE,
            x11rb::CURRENT_TIME,
        )?;

        if grab_pointer.reply()?.status == GrabStatus::SUCCESS {
            debug!("pointer grabbed successfully");
            self.pointer_grab_retry_count.set(0);
        } else {
            let new_count = self.pointer_grab_retry_count.get() + 1;
            if new_count < MAX_GRAB_RETRIES {
                self.pointer_grab_retry_count.set(new_count);
                timer.set_timer(10)?;
            } else {
                warn!("failed to grab pointer after {MAX_GRAB_RETRIES} retries");
                self.pointer_grab_retry_count.set(0);
            }
        }
        Ok(())
    }

    pub fn cancel_grab_retries(
        &self,
        keyboard_timer: &TimerfdSource,
        pointer_timer: &TimerfdSource,
    ) -> Result<()> {
        debug!("canceling grab retries");

        keyboard_timer.disarm()?;
        self.keyboard_grab_retry_count.set(0);
        pointer_timer.disarm()?;
        self.pointer_grab_retry_count.set(0);

        Ok(())
    }

    pub fn ungrab_input(&self) -> Result<()> {
        debug!("ungrabbing keyboard");
        let ungrab_keyboard = self.conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
        ungrab_keyboard.check()?;

        debug!("ungrabbing pointer");
        let ungrab_pointer = self.conn.ungrab_pointer(x11rb::CURRENT_TIME)?;
        ungrab_pointer.check()?;

        Ok(())
    }

    pub fn enable_events(&self) -> Result<()> {
        debug!("set event mask to {:?}", self.shown_win_event_mask);
        self.conn.change_window_attributes(
            self.win_id.get(),
            &ChangeWindowAttributesAux::new().event_mask(self.shown_win_event_mask),
        )?;

        Ok(())
    }

    pub fn disable_events(&self) -> Result<()> {
        debug!("set event mask to {:?}", self.hidden_win_event_mask);
        self.conn.change_window_attributes(
            self.win_id.get(),
            &ChangeWindowAttributesAux::new().event_mask(self.hidden_win_event_mask),
        )?;

        Ok(())
    }

    pub fn get_current_win_pos(&self) -> (i16, i16) {
        self.win_pos.get()
    }

    pub fn is_win_placed_above_pointer(&self) -> bool {
        self.win_placed_above_pointer.get()
    }

    fn calculate_window_pos(&self) -> Result<(i16, i16, bool)> {
        let X11Window {
            conn,
            screen,
            atoms,
            win_opened_pointer,
            config,
            ..
        } = self;
        let LayoutConfig {
            window_dimensions,
            pointer_gap,
            screen_edge_gap,
            ..
        } = config.layout;
        let width = window_dimensions.width as i32;
        let height = window_dimensions.height as i32;

        let pointer_pos = win_opened_pointer.get();
        let px = pointer_pos.0 as i32;
        let py = pointer_pos.1 as i32;

        let monitors = conn.randr_get_monitors(screen.root, true)?.reply()?;
        let pointer_monitor = monitors.monitors.iter().find(|m| {
            px >= m.x as i32
                && px < m.x as i32 + m.width as i32
                && py >= m.y as i32
                && py < m.y as i32 + m.height as i32
        });

        let desktop_viewport = get_current_desktop_viewport(conn, screen, atoms)?;
        let focused_monitor = desktop_viewport.and_then(|dv| {
            monitors.monitors.iter().find(|m| {
                (dv.x as i64) >= m.x as i64
                    && (dv.x as i64) < m.x as i64 + m.width as i64
                    && (dv.y as i64) >= m.y as i64
                    && (dv.y as i64) < m.y as i64 + m.height as i64
            })
        });

        for (position_mode, is_fallback) in config
            .window_position_mode
            .iter()
            .map(|m| (m, false))
            .chain(once((&WindowPositionMode::Monitor, true)))
        {
            let position_result = match position_mode {
                WindowPositionMode::Monitor => {
                    Self::position_by_monitor(focused_monitor, width, height)
                }
                WindowPositionMode::Pointer => Self::position_by_location(
                    &monitors.monitors,
                    (px, py),
                    width,
                    height,
                    pointer_gap,
                    screen_edge_gap,
                ),
                WindowPositionMode::ActivePointer => {
                    let pointer_visible = self.pointer_visible()?;
                    let pointer_in_focused_monitor = focused_monitor
                        .zip(pointer_monitor)
                        .map(|(fm, pm)| fm.name == pm.name)
                        .unwrap_or(false);
                    if pointer_visible && pointer_in_focused_monitor {
                        Self::position_by_location(
                            &monitors.monitors,
                            (px, py),
                            width,
                            height,
                            pointer_gap,
                            screen_edge_gap,
                        )
                    } else {
                        Err(if !pointer_visible {
                            anyhow!("pointer is not visible")
                        } else {
                            anyhow!("pointer is not in focused monitor")
                        })
                    }
                }
            };

            match position_result {
                Ok(pos) => {
                    debug!(
                        "{}window position mode {position_mode:?} is available, using its result",
                        if is_fallback { "fallback " } else { "" }
                    );
                    return Ok(pos);
                }
                Err(err) => {
                    debug!(
                        "{}window position mode {position_mode:?} is not available: {err}",
                        if is_fallback { "fallback " } else { "" }
                    );
                }
            }
        }

        debug!("no window position modes are available, falling back to zero-value position");
        Ok((0, 0, false))
    }

    fn pointer_visible(&self) -> Result<bool> {
        let reply = self.conn.xfixes_get_cursor_image()?.reply()?;
        let pixels = reply.cursor_image;
        let all_transparent = pixels.iter().all(|&p| (p & 0xff00_0000) == 0);

        Ok(!all_transparent)
    }

    fn position_by_monitor(
        monitor: Option<&randr::MonitorInfo>,
        win_width: i32,
        win_height: i32,
    ) -> Result<(i16, i16, bool)> {
        monitor
            .map(|m| {
                let x = (m.width as i32 - win_width) / 2 + m.x as i32;
                let y = (m.height as i32 - win_height) / 2 + m.y as i32;
                (
                    x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    false,
                )
            })
            .ok_or_else(|| anyhow!("no monitor for positioning"))
    }

    fn position_by_location(
        monitors: &[randr::MonitorInfo],
        (x, y): (i32, i32),
        win_width: i32,
        win_height: i32,
        location_gap: i32,
        monitor_edge_gap: i32,
    ) -> Result<(i16, i16, bool)> {
        let Some(monitor) = monitors.iter().find(|m| {
            x >= m.x as i32
                && x < m.x as i32 + m.width as i32
                && y >= m.y as i32
                && y < m.y as i32 + m.height as i32
        }) else {
            bail!("location ({x}, {y}) is outside of all monitors");
        };
        let (mx, my, mw, mh) = (
            monitor.x as i32,
            monitor.y as i32,
            monitor.width as i32,
            monitor.height as i32,
        );

        let min_x = mx + monitor_edge_gap;
        let max_x = (mx + mw - win_width - monitor_edge_gap).max(min_x);
        let min_y = my + monitor_edge_gap;
        let max_y = (my + mh - win_height - monitor_edge_gap).max(min_y);

        let place_right = x + win_width + location_gap <= mx + mw - location_gap;
        let place_below = y + win_height + location_gap <= my + mh - location_gap;

        let x = (if place_right {
            x + location_gap
        } else {
            x - win_width - location_gap
        })
        .clamp(min_x, max_x);
        let y = (if place_below {
            y + location_gap
        } else {
            y - win_height - location_gap
        })
        .clamp(min_y, max_y);

        Ok((
            x.clamp(i16::MIN.into(), i16::MAX.into()) as i16,
            y.clamp(i16::MIN.into(), i16::MAX.into()) as i16,
            !place_below,
        ))
    }
}

fn get_current_desktop_viewport(
    conn: &XCBConnection,
    screen: &Screen,
    atoms: &Atoms,
) -> Result<Option<Viewport>> {
    let reply = conn
        .get_property(
            false,
            screen.root,
            atoms._NET_CURRENT_DESKTOP,
            AtomEnum::CARDINAL,
            0,
            1,
        )?
        .reply()?;

    if reply.format == 32 && !reply.value.is_empty() {
        let current_desktop = u32::from_ne_bytes(reply.value[0..4].try_into()?) as usize;
        let mut desktop_viewports = get_desktop_viewports(conn, screen, atoms)?;
        if current_desktop < desktop_viewports.len() {
            return Ok(Some(desktop_viewports.swap_remove(current_desktop)));
        }
    }

    Ok(None)
}

fn get_desktop_viewports(
    conn: &XCBConnection,
    screen: &Screen,
    atoms: &Atoms,
) -> Result<Vec<Viewport>> {
    let reply = conn
        .get_property(
            false,
            screen.root,
            atoms._NET_DESKTOP_VIEWPORT,
            AtomEnum::CARDINAL,
            0,
            u32::MAX,
        )?
        .reply()?;

    if reply.format != 32 {
        return Ok(vec![]);
    }

    let mut values = Vec::new();
    for chunk in reply.value.as_chunks::<4>().0 {
        values.push(u32::from_ne_bytes(*chunk));
    }

    let viewports = values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Viewport {
            x: pair[0],
            y: pair[1],
        })
        .collect::<Vec<_>>();

    Ok(viewports)
}

fn find_window_under_pointer(
    conn: &XCBConnection,
    pointer_child_win: Window,
    wm_state_atom: u32,
) -> Result<Option<Window>> {
    if pointer_child_win == 0 {
        return Ok(None);
    }

    let prop = conn
        .get_property(false, pointer_child_win, wm_state_atom, AtomEnum::ANY, 0, 0)?
        .reply()?;

    // this window has WM_STATE
    if prop.type_ != 0 {
        return Ok(Some(pointer_child_win));
    }

    let tree = conn.query_tree(pointer_child_win)?.reply()?;
    for &child in &tree.children {
        if let Some(result) = find_window_under_pointer(conn, child, wm_state_atom)? {
            return Ok(Some(result));
        }
    }

    Ok(None)
}
