use std::{
    cell::RefCell,
    collections::HashMap,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use log::{debug, info, trace, warn};
use x11rb::protocol::xproto::*;
use x11rb::protocol::{Event, xtest::ConnectionExt as _};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use xkeysym::Keysym;

use crate::{
    config::{Config, KeyStroke, Modifier},
    keymap_spec::PasteModifier,
    selection::SelectionType,
    selection_item::SelectionData,
    utils::{get_x11_window_class, is_plaintext_mime},
    x11_key_converter::X11KeyConverter,
    x11_window::X11Window,
};

const OVERDUE_TIMEOUT: Duration = Duration::from_secs(3);
const INCR_CHUNK_SIZE: usize = 1024 * 1024 - 1;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        PRIMARY,
        CLIPBOARD,

        INCR,
        TARGETS,
    }
}

#[derive(Debug)]
enum IncrPasteTaskState {
    TransferingIncr {
        target: u32,
        data_atom_name: String,
        offset: usize,
        modifier: Option<PasteModifier>,
    },
}

#[derive(Debug)]
struct PasteTask<S> {
    state: S,
    last_update: Instant,
}

impl<S> PasteTask<S> {
    fn new(state: S) -> Self {
        PasteTask {
            state,
            last_update: Instant::now(),
        }
    }
}

pub struct PasteServer<'a> {
    window: &'a X11Window<'a>,
    screen: &'a Screen,
    key_converter: &'a X11KeyConverter<'a>,
    config: &'a Config,
    selection_atom: Atom,
    atoms: Atoms,
    paste_data: Option<SelectionData>,
    pending_modifier: Option<PasteModifier>,
    incr_tasks: HashMap<(Window, Atom), PasteTask<IncrPasteTaskState>>,
    mime_atoms: RefCell<HashMap<String, Atom>>,
}

impl<'a> PasteServer<'a> {
    pub fn new(
        window: &'a X11Window,
        key_converter: &'a X11KeyConverter,
        selection_type: SelectionType,
        config: &'a Config,
    ) -> Result<Self> {
        let conn = &window.conn;
        let atoms = Atoms::new(conn)?.reply()?;
        let selection_atom = match selection_type {
            SelectionType::PRIMARY => atoms.PRIMARY,
            SelectionType::CLIPBOARD => atoms.CLIPBOARD,
        };

        Ok(PasteServer {
            window,
            screen: &window.screen,
            key_converter,
            config,
            selection_atom,
            atoms,
            paste_data: None,
            pending_modifier: None,
            incr_tasks: HashMap::new(),
            mime_atoms: RefCell::new(HashMap::new()),
        })
    }

    pub fn handle_event(&mut self, event: &Event) -> Result<bool> {
        let conn = &self.window.conn;

        let handled = 'blk: {
            match event {
                // Handle paste request
                Event::SelectionRequest(ev) => {
                    debug!(
                        "paste request received from requestor {} for target {}",
                        ev.requestor, ev.target
                    );

                    let modifier = self.pending_modifier.take();

                    let reply = |property| -> Result<()> {
                        conn.send_event(
                            false,
                            ev.requestor,
                            EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: SELECTION_NOTIFY_EVENT,
                                sequence: ev.sequence,
                                time: ev.time,
                                requestor: ev.requestor,
                                selection: ev.selection,
                                target: ev.target,
                                property,
                            },
                        )?
                        .check()?;
                        Ok(())
                    };

                    let property = if ev.property == x11rb::NONE {
                        debug!("obsolete client detected");
                        ev.target
                    } else {
                        ev.property
                    };
                    if property == x11rb::NONE {
                        warn!("invalid paste request: no property provided to place the data");
                        reply(x11rb::NONE)?;
                        break 'blk true;
                    }

                    let reply = |reply_property| {
                        if reply_property == x11rb::NONE {
                            conn.delete_property(ev.requestor, property)?.check()?;
                        }
                        reply(reply_property)
                    };

                    if ev.selection != self.selection_atom {
                        debug!("unsupported selection type: {}", ev.selection);
                        reply(x11rb::NONE)?;
                        break 'blk true;
                    }
                    let Some(paste_data) = self.paste_data.as_ref() else {
                        debug!("nothing to paste: no paste data");
                        reply(x11rb::NONE)?;
                        break 'blk true;
                    };

                    let mut supported_atoms = Vec::new();
                    supported_atoms.push(self.atoms.TARGETS);
                    let mut requested_data = None;
                    for (atom_name, data) in paste_data.iter() {
                        let atom =
                            get_or_create_mime_atom(conn, self.mime_atoms.get_mut(), atom_name)?;
                        if atom != x11rb::NONE {
                            supported_atoms.push(atom);
                        }

                        if atom == ev.target {
                            requested_data = Some((data, atom_name));
                        }
                    }

                    if !supported_atoms.contains(&ev.target) {
                        debug!("unsupported target: {}", ev.target);
                        reply(x11rb::NONE)?;
                        break 'blk true;
                    }

                    if ev.target == self.atoms.TARGETS {
                        debug!("responding to paste request with TARGETS");
                        conn.change_property32(
                            PropMode::REPLACE,
                            ev.requestor,
                            property,
                            AtomEnum::ATOM,
                            &supported_atoms,
                        )?
                        .check()?;
                        self.pending_modifier = modifier; // keep the modifier for the next real target
                        reply(property)?;
                        break 'blk true;
                    }

                    info!(
                        "transfering selection to requestor {} with atom {}",
                        ev.requestor, property
                    );

                    let (raw_data, atom_name) = requested_data.unwrap();
                    let data = if modifier.is_some_and(|m| m.trim) {
                        if is_plaintext_mime(atom_name)
                            && let Ok(text_data) = str::from_utf8(raw_data)
                        {
                            text_data.trim().as_bytes()
                        } else {
                            debug!("trim-pasting on a non-text item");
                            raw_data
                        }
                    } else {
                        raw_data
                    };
                    if data.len() > INCR_CHUNK_SIZE {
                        debug!(
                            "starting paste request INCR transfer for {} bytes",
                            data.len()
                        );
                        conn.change_window_attributes(
                            ev.requestor,
                            &ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE),
                        )?;
                        conn.change_property32(
                            PropMode::REPLACE,
                            ev.requestor,
                            property,
                            self.atoms.INCR,
                            &[u32::try_from(data.len()).unwrap_or(u32::MAX)],
                        )?
                        .check()?;

                        self.incr_tasks.insert(
                            (ev.requestor, property),
                            PasteTask::new(IncrPasteTaskState::TransferingIncr {
                                target: ev.target,
                                data_atom_name: atom_name.to_string(),
                                offset: 0,
                                modifier,
                            }),
                        );

                        reply(property)?;
                        break 'blk true;
                    }

                    info!(
                        "responded to requestor {} paste request with selection ({atom_name})",
                        ev.requestor
                    );
                    conn.change_property8(
                        PropMode::REPLACE,
                        ev.requestor,
                        property,
                        ev.target,
                        data,
                    )?
                    .check()?;
                    reply(property)?;

                    if modifier.is_some_and(|m| m.and_enter) {
                        let (key, _, keycode) =
                            get_input_utils(conn, self.screen, self.key_converter);

                        // Release previously pressed Enter key if it's mapped as paste action
                        key(KEY_RELEASE_EVENT, keycode(Keysym::Return)?)?;

                        key(KEY_PRESS_EVENT, keycode(Keysym::Return)?)?;
                        key(KEY_RELEASE_EVENT, keycode(Keysym::Return)?)?;
                    }

                    true
                }

                // Handle INCR paste request
                Event::PropertyNotify(ev) => {
                    if ev.state != Property::DELETE {
                        trace!("ignoring irrelevant property state change: {:?}", ev.state);
                        break 'blk false;
                    }

                    let Some(task) = self.incr_tasks.get_mut(&(ev.window, ev.atom)) else {
                        trace!(
                            "no active INCR transfer task for window {} and atom {}",
                            ev.window, ev.atom
                        );
                        break 'blk false;
                    };

                    match task.state {
                        IncrPasteTaskState::TransferingIncr {
                            target,
                            ref data_atom_name,
                            ref mut offset,
                            modifier,
                        } => {
                            let end_transfering =
                                |incr_paste_tasks: &mut HashMap<_, _>| -> Result<()> {
                                    incr_paste_tasks.remove(&(ev.window, ev.atom));
                                    conn.change_window_attributes(
                                        ev.window,
                                        &ChangeWindowAttributesAux::new()
                                            .event_mask(EventMask::NO_EVENT),
                                    )?;
                                    Ok(())
                                };

                            if let Some(data) = self
                                .paste_data
                                .as_ref()
                                .and_then(|data| data.get(data_atom_name))
                            {
                                let end = offset.saturating_add(INCR_CHUNK_SIZE).min(data.len());
                                let chunk = &data[*offset..end];

                                if *offset == end {
                                    info!(
                                        "responded to requestor {} paste request with selection ({data_atom_name})",
                                        ev.window
                                    );
                                    end_transfering(&mut self.incr_tasks)?;

                                    if modifier.is_some_and(|m| m.and_enter) {
                                        let (key, _, keycode) =
                                            get_input_utils(conn, self.screen, self.key_converter);

                                        // Release previously pressed Enter key if it's mapped as paste action
                                        key(KEY_RELEASE_EVENT, keycode(Keysym::Return)?)?;

                                        key(KEY_PRESS_EVENT, keycode(Keysym::Return)?)?;
                                        key(KEY_RELEASE_EVENT, keycode(Keysym::Return)?)?;
                                    }
                                } else {
                                    debug!(
                                        "continuing INCR transfer with {} bytes remaining",
                                        data.len() - end
                                    );
                                    *offset = end;
                                }

                                conn.change_property8(
                                    PropMode::REPLACE,
                                    ev.window,
                                    ev.atom,
                                    target,
                                    chunk,
                                )?
                                .check()?;
                            } else {
                                warn!(
                                    "paste selection with target {data_atom_name} not found, stopping INCR transfer"
                                );
                                end_transfering(&mut self.incr_tasks)?;
                                conn.change_property8(
                                    PropMode::REPLACE,
                                    ev.window,
                                    ev.atom,
                                    target,
                                    &[],
                                )?
                                .check()?;
                            }
                        }
                    }

                    true
                }

                Event::SelectionClear(ev)
                    if ev.owner == self.window.win_id.get() && self.paste_data.is_some() =>
                {
                    info!("lost selection ownership");
                    true
                }

                _ => false,
            }
        };

        self.purge_overdue_tasks();

        Ok(handled)
    }

    pub fn claim_selection_ownership(&mut self, data: SelectionData) -> Result<()> {
        let paste_window = self.window.win_id.get();
        self.window
            .conn
            .set_selection_owner(paste_window, self.selection_atom, x11rb::CURRENT_TIME)?
            .check()?;
        self.paste_data = Some(data);

        Ok(())
    }

    fn purge_overdue_tasks(&mut self) {
        let now = Instant::now();

        let (paste_kept, paste_removed): (HashMap<_, _>, HashMap<_, _>) = self
            .incr_tasks
            .drain()
            .partition(|(_, task)| now.duration_since(task.last_update) < OVERDUE_TIMEOUT);
        self.incr_tasks = paste_kept;

        if !paste_removed.is_empty() {
            warn!("purging overdue paste tasks: {:?}", paste_removed.keys());
        }
    }

    pub fn paste(
        &mut self,
        data: SelectionData,
        original_pointer: (i16, i16, Option<Window>),
        modifier: PasteModifier,
    ) -> Result<()> {
        let conn = &self.window.conn;
        let paste_window = self.window.win_id.get();

        let focused_window = conn.get_input_focus()?.reply()?.focus;
        if focused_window == paste_window {
            warn!("trying to paste into itself");
            return Ok(());
        }

        // Claim ownership before sending paste key events so that the
        // requestor can't request selection before we have the ownership
        self.claim_selection_ownership(data)?;

        let (key, move_pointer, keycode) = get_input_utils(conn, self.screen, self.key_converter);

        // Prevent holding modifiers from interfering with pasting keymap
        key(KEY_RELEASE_EVENT, keycode(Keysym::Control_L)?)?;
        key(KEY_RELEASE_EVENT, keycode(Keysym::Control_R)?)?;
        key(KEY_RELEASE_EVENT, keycode(Keysym::Shift_L)?)?;
        key(KEY_RELEASE_EVENT, keycode(Keysym::Shift_R)?)?;
        key(KEY_RELEASE_EVENT, keycode(Keysym::Alt_L)?)?;
        key(KEY_RELEASE_EVENT, keycode(Keysym::Alt_R)?)?;
        key(KEY_RELEASE_EVENT, keycode(Keysym::Super_L)?)?;
        key(KEY_RELEASE_EVENT, keycode(Keysym::Super_R)?)?;

        if self.selection_atom == self.atoms.CLIPBOARD {
            let app_paste_keymaps = &self.config.app_paste_keymaps;
            // Focus may be PointerRoot/None or the window may already be gone;
            // fall back to the default keymap instead of failing the paste.
            let window_class = get_x11_window_class(conn, focused_window)
                .inspect_err(|e| {
                    warn!("failed to get class of focused window {focused_window}: {e}")
                })
                .ok()
                .flatten();
            let keymap = if let Some((instance_name, class_name)) = window_class
                && let Some(keymap) = app_paste_keymaps
                    .get(&instance_name)
                    .or_else(|| app_paste_keymaps.get(&class_name))
            {
                keymap
            } else {
                &vec![KeyStroke {
                    key: 'v' as u32,
                    modifiers: vec![Modifier::Control],
                }]
            };

            info!("pasting into {focused_window} using keymap: {keymap:?}");
            for key_stroke in keymap {
                for modifier in &key_stroke.modifiers {
                    key(KEY_PRESS_EVENT, keycode((*modifier).into())?)?;
                }
                key(KEY_PRESS_EVENT, keycode(Keysym::new(key_stroke.key))?)?;
                key(KEY_RELEASE_EVENT, keycode(Keysym::new(key_stroke.key))?)?;
                for modifier in key_stroke.modifiers.iter().rev() {
                    key(KEY_RELEASE_EVENT, keycode((*modifier).into())?)?;
                }
            }
        } else if self.selection_atom == self.atoms.PRIMARY {
            info!(
                "pasting into {focused_window} using middle mouse button at ({}, {})",
                original_pointer.0, original_pointer.1
            );
            let pointer_current_pos = conn.query_pointer(self.screen.root)?.reply()?;

            // Some windows (e.g., Firefox) don't receive paste event if memoni middle-clickes immediately
            // after memoni window closes. For these, we can make memoni wait a moment before clicking.
            let need_delay = original_pointer
                .2
                .and_then(|win| {
                    get_x11_window_class(conn, win)
                        .inspect_err(|e| warn!("failed to get class of window under pointer: {e}"))
                        .ok()
                        .flatten()
                })
                .map(|(instance_name, class_name)| {
                    let instance_matched = self.config
                        .middle_click_delay_patterns
                        .iter()
                        .any(|re| re.is_match(&instance_name));
                    if instance_matched {
                        debug!("applying middle click delay workaround for window with instance '{instance_name}'");
                        return true;
                    }

                    let class_matched = self.config
                        .middle_click_delay_patterns
                        .iter()
                        .any(|re| re.is_match(&class_name));
                    if class_matched {
                        debug!("applying middle click delay workaround for window with class '{class_name}'");
                        return true;
                    }

                    false
                })
                .unwrap_or(false);

            if need_delay {
                thread::sleep(Duration::from_millis(150));
            }

            move_pointer(original_pointer.0, original_pointer.1)?;
            key(BUTTON_PRESS_EVENT, 2)?; // middle mouse button

            if need_delay {
                conn.sync()?;
                thread::sleep(Duration::from_millis(2));
            }

            key(BUTTON_RELEASE_EVENT, 2)?;
            move_pointer(pointer_current_pos.root_x, pointer_current_pos.root_y)?;
        }
        conn.sync()?;

        self.pending_modifier = Some(modifier);

        Ok(())
    }
}

fn get_or_create_mime_atom(
    conn: &XCBConnection,
    mime_atoms: &mut HashMap<String, Atom>,
    name: &str,
) -> Result<Atom> {
    if let Some(atom) = mime_atoms.get(name) {
        return Ok(*atom);
    }

    let atom = conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
    mime_atoms.insert(name.to_string(), atom);
    Ok(atom)
}

#[allow(clippy::type_complexity)]
fn get_input_utils(
    conn: &XCBConnection,
    screen: &Screen,
    key_converter: &X11KeyConverter,
) -> (
    impl Fn(u8, u8) -> Result<()>,
    impl Fn(i16, i16) -> Result<()>,
    impl Fn(Keysym) -> Result<u8>,
) {
    let key = |type_, code| {
        conn.xtest_fake_input(type_, code, x11rb::CURRENT_TIME, screen.root, 1, 1, 0)?;
        Ok(())
    };
    let move_pointer = |x, y| {
        conn.xtest_fake_input(
            MOTION_NOTIFY_EVENT,
            0,
            x11rb::CURRENT_TIME,
            screen.root,
            x,
            y,
            0,
        )?;
        Ok(())
    };
    let keycode = |keysym| {
        key_converter
            .keysym_to_keycode(keysym)
            .map(|kc| kc.raw() as u8)
            .ok_or_else(|| {
                anyhow!(
                    "invalid key provided: {}",
                    keysym.name().unwrap_or(&format!("<code {}>", keysym.raw()))
                )
            })
    };

    (key, move_pointer, keycode)
}
