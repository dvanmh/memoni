/*
    Heavily modified from Ringboard by Manh Vu on 2025-11-18
    Original under Apache-2.0. See LICENSES/apache-ringboard
    Original file: https://github.com/SUPERCILEX/clipboard-history/tree/c06ebd0837b4e42b9f9bbb8fbf41612365b76aaa/x11/src/main.rs
*/

extern crate x11rb;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fmt, mem,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use bincode::{Decode, Encode};
use log::{debug, error, info, trace, warn};
use x11rb::protocol::{
    Event,
    xfixes::{SelectionEvent, SelectionEventMask},
    xproto::{ConnectionExt as _, *},
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{
    connection::Connection,
    connection::RequestConnection as _,
    protocol::{xfixes, xtest::ConnectionExt as _},
};
use xkeysym::Keysym;

use crate::{
    config::{Config, KeyStroke, Modifier},
    keymap_action::PasteModifier,
    ordered_hash_map::OrderedHashMap,
    selection_item::{SelectionData, SelectionItem},
    transfer_window_pool::{TransferWindow, TransferWindowPool},
    utils::{image_mime_score, is_image_mime, is_plaintext_mime, plaintext_mime_score},
    x11_key_converter::X11KeyConverter,
    x11_window::X11Window,
};

const HASH_SEED: usize = 0xfd9aadcf54cc0f35;
const BINCODE_CONFIG: bincode::config::Configuration = bincode::config::standard();
const OVERDUE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_INCR_SIZE: usize = 10 * 1024 * 1024;
const INCR_CHUNK_SIZE: usize = 1024 * 1024 - 1;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        PRIMARY,
        CLIPBOARD,

        INCR,
        TIMESTAMP,
        TARGETS,
        SAVE_TARGETS,
        MULTIPLE,

        DELETE,
        INSERT_PROPERTY,
        INSERT_SELECTION,

        _NET_WM_NAME,
    }
}

type Owner = u32;

#[derive(Debug, Default, Encode, Decode)]
pub struct SelectionMetadata {
    pub pinned_count: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SelectionType {
    PRIMARY,
    CLIPBOARD,
}
impl fmt::Display for SelectionType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug)]
enum RequestTaskState {
    TargetsRequest,
    PendingSelection {
        mimes: HashMap<Atom, String>,
        data: SelectionData,
    },
    PendingIncr {
        mimes: HashMap<Atom, String>,
        data: SelectionData,
        current_mime_atom: Atom,
        current_mime_name: String,
        buffer: Vec<u8>,
    },
}

#[derive(Debug)]
enum IncrPasteTaskState {
    TransferingIncr {
        target: u32,
        item_id: u64,
        data_atom_name: String,
        offset: usize,
        modifier: Option<PasteModifier>,
    },
}

#[derive(Debug)]
struct Task<S, M = ()> {
    state: S,
    metadata: M,
    last_update: Instant,
}

impl<S, M> Task<S, M> {
    fn new(state: S, metadata: M) -> Self {
        Task {
            state,
            metadata,
            last_update: Instant::now(),
        }
    }

    fn set_state(&mut self, state: S) {
        self.state = state;
        self.last_update = Instant::now();
    }
}

pub struct Selection<'a> {
    pub items: OrderedHashMap<u64, SelectionItem>,
    pub metadata: SelectionMetadata,

    window: &'a X11Window<'a>,
    screen: &'a Screen,
    key_converter: &'a X11KeyConverter<'a>,
    config: &'a Config,
    merge_consecutive_similar_items: bool,
    selection_atom: Atom,
    atoms: Atoms,
    request_tasks: HashMap<Window, Task<RequestTaskState, (Atom, Owner)>>,
    incr_paste_tasks: HashMap<(Window, Atom), Task<IncrPasteTaskState>>,
    transfer_windows: TransferWindowPool<'a>,
    mime_atoms: RefCell<HashMap<String, Atom>>,
    paste_item_id: Option<u64>,
    next_paste_modifier: Option<PasteModifier>,
    prev_item_metadata: Option<(u32, Instant, bool)>,
}

impl<'a> Selection<'a> {
    pub fn new(
        initial_data: (OrderedHashMap<u64, SelectionItem>, SelectionMetadata),
        window: &'a X11Window,
        key_converter: &'a X11KeyConverter,
        selection_type: SelectionType,
        config: &'a Config,
        merge_consecutive_similar_items: bool,
    ) -> Result<Self> {
        let conn = &window.conn;
        let root = window.screen.root;
        let atoms = Atoms::new(conn)?.reply()?;
        let selection_atom = match selection_type {
            SelectionType::PRIMARY => atoms.PRIMARY,
            SelectionType::CLIPBOARD => atoms.CLIPBOARD,
        };

        debug!("ensuring XFixes exists");
        conn.prefetch_extension_information(xfixes::X11_EXTENSION_NAME)?;
        conn.extension_information(xfixes::X11_EXTENSION_NAME)?
            .context("XFixes not found")?;
        xfixes::query_version(conn, 5, 0)?.reply()?;
        let event_mask = SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE;
        xfixes::select_selection_input(conn, root, selection_atom, event_mask)?;

        Ok(Selection {
            items: initial_data.0,
            metadata: initial_data.1,
            window,
            screen: &window.screen,
            key_converter,
            config,
            merge_consecutive_similar_items,
            selection_atom,
            atoms,
            request_tasks: HashMap::new(),
            incr_paste_tasks: HashMap::new(),
            transfer_windows: TransferWindowPool::new(conn, root, selection_type)?,
            mime_atoms: RefCell::new(HashMap::new()),
            paste_item_id: None,
            next_paste_modifier: None,
            prev_item_metadata: None,
        })
    }

    pub fn handle_event(
        &mut self,
        event: &Event,
    ) -> Result<Option<(Option<&SelectionItem>, Vec<SelectionItem>)>> {
        let conn = &self.window.conn;
        let paste_window = self.window.win_id.get();
        let atoms = self.atoms;

        'blk: {
            match event {
                // Capture copied data
                Event::XfixesSelectionNotify(ev) => {
                    if ev.owner == paste_window {
                        debug!("ignoring selection notification from ourselves");
                        break 'blk;
                    }

                    match ev.subtype {
                        SelectionEvent::SET_SELECTION_OWNER => {
                            if ev.owner == x11rb::NONE {
                                debug!("selection explicitly cleared, ignoring");
                                break 'blk;
                            }

                            info!("selection notification received from owner {}", ev.owner);
                            let transfer_window = self.transfer_windows.get()?;
                            info!("requesting selection with transfer window: {transfer_window:?}");
                            conn.convert_selection(
                                transfer_window.id,
                                ev.selection,
                                atoms.TARGETS,
                                transfer_window.atom,
                                x11rb::CURRENT_TIME,
                            )?
                            .check()?;

                            self.request_tasks.insert(
                                transfer_window.id,
                                Task::new(
                                    RequestTaskState::TargetsRequest,
                                    (transfer_window.atom, ev.owner),
                                ),
                            );
                        }
                        SelectionEvent::SELECTION_WINDOW_DESTROY
                        | SelectionEvent::SELECTION_CLIENT_CLOSE => {
                            let Some((&item_id, _)) =
                                self.items.get_by_index(self.metadata.pinned_count)
                            else {
                                debug!(
                                    "selection owner died but no items to serve, leaving selection cleared"
                                );
                                break 'blk;
                            };

                            info!(
                                "selection owner died ({:?}), taking over ownership with item {item_id}",
                                ev.subtype
                            );
                            conn.set_selection_owner(
                                paste_window,
                                self.selection_atom,
                                x11rb::CURRENT_TIME,
                            )?
                            .check()?;
                            self.paste_item_id = Some(item_id);
                        }
                        _ => {}
                    }
                }
                Event::SelectionNotify(ev) => {
                    let transfer_window = ev.requestor;
                    let Some(task) = self.request_tasks.get_mut(&transfer_window) else {
                        warn!(
                            "ignoring selection notification to unknown requestor: {transfer_window}",
                        );
                        break 'blk;
                    };
                    let (transfer_atom, owner) = task.metadata;

                    let property = if ev.property == x11rb::NONE {
                        None
                    } else {
                        let prop = conn.get_property(
                            true,
                            ev.requestor,
                            ev.property,
                            GetPropertyType::ANY,
                            0,
                            u32::MAX,
                        )?;
                        conn.flush()?;
                        Some(prop)
                    };

                    match task.state {
                        RequestTaskState::TargetsRequest => {
                            debug!(
                                "targets request received for transfer window {transfer_window}",
                            );

                            let Some(property) = property else {
                                warn!("targets response cancelled");
                                break 'blk;
                            };

                            let property = property.reply()?;
                            if property.type_ == atoms.INCR {
                                warn!("ignoring abusive TARGETS property: {property:?}");
                                break 'blk;
                            }

                            let Some(value) = property.value32() else {
                                error!("invalid TARGETS property value format: {property:?}");
                                break 'blk;
                            };

                            let mut atom_cookies = Vec::new();
                            for atom in value {
                                if [
                                    // Special targets
                                    atoms.TIMESTAMP,
                                    atoms.TARGETS,
                                    atoms.SAVE_TARGETS,
                                    atoms.MULTIPLE,
                                    // Targets with side effects
                                    atoms.DELETE,
                                    atoms.INSERT_SELECTION,
                                    atoms.INSERT_PROPERTY,
                                ]
                                .contains(&atom)
                                {
                                    continue;
                                }

                                atom_cookies.push((conn.get_atom_name(atom)?, atom));
                            }

                            let mut mimes = HashMap::new();
                            for (cookie, atom) in atom_cookies {
                                let reply = cookie.reply()?;
                                let name = str::from_utf8(&reply.name)?.to_string();
                                mimes.insert(atom, name);
                            }
                            debug!("unfiltered targets: {mimes:?}");

                            let mimes = filter_mimes(mimes);
                            if mimes.is_empty() {
                                warn!("no usable targets returned, dropping selection");
                                break 'blk;
                            }

                            info!("choosing targets: {:?}", mimes.values());
                            if let Some(&target_atom) = mimes.keys().next() {
                                conn.convert_selection(
                                    transfer_window,
                                    ev.selection,
                                    target_atom,
                                    transfer_atom,
                                    x11rb::CURRENT_TIME,
                                )?
                                .check()?;
                            }

                            task.set_state(RequestTaskState::PendingSelection {
                                mimes,
                                data: BTreeMap::new(),
                            });
                        }
                        RequestTaskState::PendingSelection {
                            ref mut mimes,
                            ref mut data,
                        } => {
                            let mime_name = mimes.remove(&ev.target);
                            debug!(
                                "pending selection target {} ({}) received for transfer window {transfer_window}",
                                ev.target,
                                mime_name
                                    .as_ref()
                                    .map(|n| format!("\"{n}\""))
                                    .unwrap_or("None".to_string())
                            );
                            let (property_type, property_value) = if let Some(property) = property {
                                let prop_rep = property.reply()?;
                                (prop_rep.type_, prop_rep.value)
                            } else {
                                (x11rb::NONE, vec![])
                            };

                            if property_type == atoms.INCR
                                && let Some(mime_name) = mime_name
                            {
                                debug!("waiting for INCR transfer");
                                let mimes = mem::take(mimes);
                                let data = mem::take(data);

                                task.set_state(RequestTaskState::PendingIncr {
                                    mimes,
                                    data,
                                    current_mime_atom: ev.target,
                                    current_mime_name: mime_name,
                                    buffer: Vec::new(),
                                });

                                break 'blk;
                            }

                            return self.process_selection_data(
                                TransferWindow {
                                    id: transfer_window,
                                    atom: transfer_atom,
                                },
                                property_value,
                                mime_name,
                                ev.target,
                                owner,
                            );
                        }
                        RequestTaskState::PendingIncr { .. } => {
                            unreachable!("PendingIncr should only be handled in PropertyNotify");
                        }
                    };
                }

                // Handle receiving INCR selection
                Event::PropertyNotify(ev)
                    if !self.incr_paste_tasks.contains_key(&(ev.window, ev.atom)) =>
                {
                    if ev.atom == self.atoms._NET_WM_NAME {
                        trace!("ignoring window name property change");
                        break 'blk;
                    }

                    if ev.state != Property::NEW_VALUE {
                        trace!("ignoring irrelevant property state change: {:?}", ev.state);
                        break 'blk;
                    }

                    let transfer_window = ev.window;
                    let Some(task) = self.request_tasks.get_mut(&transfer_window) else {
                        warn!(
                            "ignoring property notification to unknown requestor: {transfer_window}"
                        );
                        break 'blk;
                    };
                    let (transfer_atom, owner) = task.metadata;

                    let incr_task_state = if let RequestTaskState::PendingIncr {
                        mimes,
                        data,
                        current_mime_atom,
                        current_mime_name,
                        buffer,
                    } = &mut task.state
                    {
                        Some((
                            mem::take(mimes),
                            mem::take(data),
                            mem::take(buffer),
                            mem::take(current_mime_name),
                            *current_mime_atom,
                        ))
                    } else {
                        trace!("ignoring property to be processed in selection notification");
                        None
                    };

                    if let Some((mimes, data, mut buffer, current_mime_name, current_mime_atom)) =
                        incr_task_state
                    {
                        debug!(
                            "pending INCR selection target {} ({:?}) received for transfer window {transfer_window}",
                            current_mime_atom, current_mime_name
                        );

                        let property = conn.get_property(
                            true,
                            ev.window,
                            ev.atom,
                            GetPropertyType::ANY,
                            0,
                            u32::MAX,
                        )?;
                        conn.flush()?;

                        let property = property.reply()?;

                        // Empty property signals completion
                        if property.value.is_empty() {
                            conn.delete_property(ev.window, transfer_atom)?;
                            task.state = RequestTaskState::PendingSelection { mimes, data };

                            return self.process_selection_data(
                                TransferWindow {
                                    id: transfer_window,
                                    atom: transfer_atom,
                                },
                                buffer,
                                Some(current_mime_name),
                                current_mime_atom,
                                owner,
                            );
                        }

                        if buffer.len() + property.value.len() > MAX_INCR_SIZE {
                            warn!(
                                "INCR transfer exceeds size limit of {MAX_INCR_SIZE} bytes, dropping selection"
                            );
                            self.request_tasks.remove(&transfer_window);
                            break 'blk;
                        }

                        debug!("writing {} bytes for INCR transfer", property.value.len());
                        buffer.extend_from_slice(&property.value);
                        task.state = RequestTaskState::PendingIncr {
                            mimes,
                            data,
                            buffer,
                            current_mime_name,
                            current_mime_atom,
                        };
                    }
                }

                // Handle paste request
                Event::SelectionRequest(ev) => {
                    debug!(
                        "paste request received from requestor {} for target {}",
                        ev.requestor, ev.target
                    );

                    let modifier = self.next_paste_modifier;
                    self.next_paste_modifier = None;

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
                        break 'blk reply(x11rb::NONE)?;
                    }

                    let reply = |reply_property| {
                        if reply_property == x11rb::NONE {
                            conn.delete_property(ev.requestor, property)?.check()?;
                        }
                        reply(reply_property)
                    };

                    if ev.selection != self.selection_atom {
                        debug!("unsupported selection type: {}", ev.selection);
                        break 'blk reply(x11rb::NONE)?;
                    }
                    let Some(item_id) = self.paste_item_id else {
                        debug!("nothing to paste: no paste item id");
                        break 'blk reply(x11rb::NONE)?;
                    };
                    let Some(item) = self.items.get(&item_id) else {
                        debug!("nothing to paste: no paste item");
                        break 'blk reply(x11rb::NONE)?;
                    };

                    let mut supported_atoms = Vec::new();
                    supported_atoms.push(self.atoms.TARGETS);
                    let mut requested_data = None;
                    for (atom_name, data) in item.data() {
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
                        break 'blk reply(x11rb::NONE)?;
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
                        self.next_paste_modifier = modifier; // keep the modifier for the next real target
                        break 'blk reply(property)?;
                    }

                    info!(
                        "transfering selection to requestor {} with atom {}",
                        ev.requestor, property
                    );

                    let (raw_data, atom_name) = requested_data.unwrap();
                    let data = if modifier.is_some_and(|m| m.trim) {
                        if is_plaintext_mime(atom_name) {
                            trim_unicode_utf8(raw_data).unwrap()
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
                            atoms.INCR,
                            &[u32::try_from(data.len()).unwrap_or(u32::MAX)],
                        )?
                        .check()?;

                        self.incr_paste_tasks.insert(
                            (ev.requestor, property),
                            Task::new(
                                IncrPasteTaskState::TransferingIncr {
                                    target: ev.target,
                                    item_id,
                                    data_atom_name: atom_name.to_string(),
                                    offset: 0,
                                    modifier,
                                },
                                (),
                            ),
                        );

                        break 'blk reply(property)?;
                    }

                    info!(
                        "responded to requestor {} paste request with selection {} ({atom_name})",
                        ev.requestor,
                        item.id()
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
                }

                // Handle INCR paste request
                Event::PropertyNotify(ev) => {
                    if ev.state != Property::DELETE {
                        trace!("ignoring irrelevant property state change: {:?}", ev.state);
                        break 'blk;
                    }

                    let Some(task) = self.incr_paste_tasks.get_mut(&(ev.window, ev.atom)) else {
                        error!("received property notification after INCR transfer completed");
                        break 'blk;
                    };

                    match task.state {
                        IncrPasteTaskState::TransferingIncr {
                            target,
                            item_id,
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

                            if let Some(data) = &self
                                .items
                                .get(&item_id)
                                .and_then(|i| i.data().get(data_atom_name))
                            {
                                let end = offset.saturating_add(INCR_CHUNK_SIZE).min(data.len());
                                let chunk = &data[*offset..end];

                                if *offset == end {
                                    info!(
                                        "responded to requestor {} paste request with selection {} ({data_atom_name})",
                                        ev.window, item_id
                                    );
                                    end_transfering(&mut self.incr_paste_tasks)?;

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
                                    "paste selection {item_id} with target {data_atom_name} not found, stopping INCR transfer"
                                );
                                end_transfering(&mut self.incr_paste_tasks)?;
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
                }
                Event::SelectionClear(event)
                    if event.owner == paste_window && self.paste_item_id.is_some() =>
                {
                    info!("lost selection ownership");
                }
                _ => {}
            }
        }

        self.purge_overdue_tasks();

        Ok(None)
    }

    fn process_selection_data(
        &mut self,
        transfer_window: TransferWindow,
        value: Vec<u8>,
        mime_name: Option<String>,
        mime_atom: Atom,
        owner: Owner,
    ) -> Result<Option<(Option<&SelectionItem>, Vec<SelectionItem>)>> {
        let mut task = self.request_tasks.remove(&transfer_window.id).unwrap();

        let RequestTaskState::PendingSelection {
            ref mut data,
            ref mimes,
        } = task.state
        else {
            panic!(
                "Expected task.state of transfer_atom {} to be PendingSelection, but was {:?}",
                transfer_window.atom, task.state
            );
        };

        if !value.is_empty()
            && let Some(mime_name) = mime_name
        {
            data.insert(mime_name.clone(), value);
            self.mime_atoms
                .borrow_mut()
                .insert(mime_name.clone(), mime_atom);
        } else {
            warn!("dropping empty or invalid target");
        }

        if let Some(&next_atom) = mimes.keys().next() {
            self.request_tasks.insert(transfer_window.id, task);
            self.window
                .conn
                .convert_selection(
                    transfer_window.id,
                    self.selection_atom,
                    next_atom,
                    transfer_window.atom,
                    x11rb::CURRENT_TIME,
                )?
                .check()?;
            return Ok(None);
        }

        self.transfer_windows.release(transfer_window);

        if data.is_empty() {
            warn!("dropping empty selection");
            return Ok(None);
        }

        let prev_item = self.items.front();
        let new_item_id = hash_selection_data(data)?;
        let mut removed = Vec::new();

        // We only support merge plaintext items without any other type of data
        if self.merge_consecutive_similar_items
            && let Some((prev_owner, prev_time, is_previously_seen)) = self.prev_item_metadata
            && prev_owner == owner
            && prev_time.elapsed() < Duration::from_secs(1)
            // ---
            // If the item has existed before, we should not merge it
            && !is_previously_seen
            // ---
            && data.len() == 1
            && let (mime, new_text) = data.iter().next().unwrap()
            && is_plaintext_mime(mime)
            // ---
            && let Some((_, prev_item)) = prev_item
            && prev_item.data().len() == 1
            && let Some(prev_text) = prev_item.data().get(mime)
            // ---
            && (contains(new_text, prev_text) || contains(prev_text, new_text))
        {
            debug!("merging selection with the previous one");
            removed.push(self.items.pop_front().unwrap().1);
        }

        let mut is_previously_seen = false;
        let mut new_item = None;
        if self.items.contains_key(&new_item_id) {
            if self
                .items
                .iter()
                .take(self.metadata.pinned_count)
                .any(|(&id, _)| id == new_item_id)
            {
                debug!("selection is duplicated, old one is pinned, keeping old selection");
            } else {
                debug!("selection is duplicated, removing old one");
                let previous_seen_item = self.items.remove(&new_item_id).unwrap();
                self.items
                    .insert(self.metadata.pinned_count, new_item_id, previous_seen_item);
            }

            is_previously_seen = true;
        } else {
            self.items.insert(
                self.metadata.pinned_count,
                new_item_id,
                SelectionItem::create(new_item_id, mem::take(data)),
            );

            if self.items.len() > self.config.item_limit {
                let removed_map = self.items.split_off(self.config.item_limit);
                removed.extend(removed_map.into_iter().map(|(_, i)| i));
            };

            new_item = self.items.get(&new_item_id);
        }

        info!("selection transfer completed with new selection: {new_item_id}");
        self.prev_item_metadata = Some((owner, Instant::now(), is_previously_seen));
        Ok(Some((new_item, removed)))
    }

    fn purge_overdue_tasks(&mut self) {
        let now = Instant::now();

        let (request_kept, request_removed): (HashMap<_, _>, HashMap<_, _>) = self
            .request_tasks
            .drain()
            .partition(|(_, task)| now.duration_since(task.last_update) < OVERDUE_TIMEOUT);
        self.request_tasks = request_kept;

        if !request_removed.is_empty() {
            warn!(
                "purging overdue request tasks: {:?}",
                request_removed.keys()
            );
        }

        for (
            transfer_window,
            Task {
                metadata: (transfer_atom, _),
                ..
            },
        ) in request_removed
        {
            self.transfer_windows.release(TransferWindow {
                id: transfer_window,
                atom: transfer_atom,
            });
        }

        let (paste_kept, paste_removed): (HashMap<_, _>, HashMap<_, _>) = self
            .incr_paste_tasks
            .drain()
            .partition(|(_, task)| now.duration_since(task.last_update) < OVERDUE_TIMEOUT);
        self.incr_paste_tasks = paste_kept;

        if !paste_removed.is_empty() {
            warn!("purging overdue paste tasks: {:?}", paste_removed.keys());
        }
    }

    pub fn paste(
        &mut self,
        item_id: u64,
        pointer_original_pos: (i16, i16),
        modifier: PasteModifier,
    ) -> Result<()> {
        // Move paste item to the top
        let Some(item_idx) = self.items.iter().position(|(&id, _)| id == item_id) else {
            bail!("item not found: {item_id}");
        };
        if item_idx >= self.metadata.pinned_count {
            let item = self.items.remove(&item_id).unwrap();
            self.items.insert(self.metadata.pinned_count, item_id, item);
        }

        let conn = &self.window.conn;
        let paste_window = self.window.win_id.get();

        let focused_window = conn.get_input_focus()?.reply()?.focus;
        if focused_window == paste_window {
            warn!("trying to paste into itself");
            return Ok(());
        }

        conn.set_selection_owner(paste_window, self.selection_atom, x11rb::CURRENT_TIME)?
            .check()?;

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
            let window_class = get_window_class(conn, focused_window)
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
                "pasting into {focused_window} using middle mouse button at {pointer_original_pos:?}"
            );
            let pointer_current_pos = conn.query_pointer(self.screen.root)?.reply()?;
            move_pointer(pointer_original_pos.0, pointer_original_pos.1)?;

            // middle mouse button
            key(BUTTON_PRESS_EVENT, 2)?;
            key(BUTTON_RELEASE_EVENT, 2)?;

            move_pointer(pointer_current_pos.root_x, pointer_current_pos.root_y)?;
        }
        conn.flush()?;

        self.paste_item_id = Some(item_id);
        self.next_paste_modifier = Some(modifier);

        Ok(())
    }

    /// Returns true if item is pinned, false if item is unpinned
    pub fn toggle_pin(&mut self, item_id: u64) -> Result<bool> {
        let is_pinned = self
            .items
            .iter()
            .take(self.metadata.pinned_count)
            .any(|(&id, _)| id == item_id);

        let Some(item) = self.items.remove(&item_id) else {
            bail!("item not found: {item_id}");
        };

        if is_pinned {
            self.metadata.pinned_count -= 1;
            self.items.insert(self.metadata.pinned_count, item_id, item);
            Ok(false)
        } else {
            self.metadata.pinned_count += 1;
            self.items.push_front(item_id, item);
            Ok(true)
        }
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

fn filter_mimes(mimes: HashMap<Atom, String>) -> HashMap<Atom, String> {
    let mut filtered_mimes = HashMap::new();
    let mut plain: Option<(Atom, &str)> = None;
    let mut plain_score = 0;
    let mut image: Option<(Atom, &str)> = None;
    let mut image_score = 0;

    for (atom, mime) in mimes.iter() {
        if let Some(score) = plaintext_mime_score(mime) {
            if plain.is_none_or(|_| score > plain_score) {
                plain = Some((*atom, mime));
                plain_score = score;
            }
        } else if is_image_mime(mime) {
            let score = image_mime_score(mime);
            if image.is_none_or(|_| score > image_score) {
                image = Some((*atom, mime));
                image_score = score;
            }
        } else if mime == "x-kde-passwordManagerHint" {
            debug!("selection type is password, filtering out all targets");
            filtered_mimes.drain();
            return filtered_mimes;
        } else {
            filtered_mimes.insert(*atom, mime.to_string());
        }
    }

    if let Some((atom, mime)) = plain {
        filtered_mimes.insert(atom, mime.to_string());
    }
    if let Some((atom, mime)) = image {
        filtered_mimes.insert(atom, mime.to_string());
    }

    filtered_mimes
}

fn get_window_class(conn: &XCBConnection, window: Window) -> Result<Option<(String, String)>> {
    let reply: GetPropertyReply = conn
        .get_property(
            false,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            0,
            u32::MAX,
        )?
        .reply()?;

    if reply.value_len == 0 {
        return Ok(None);
    }

    let value = String::from_utf8_lossy(&reply.value).into_owned();
    let mut parts = value.split('\0');

    let instance_name = parts.next().unwrap_or("").to_string();
    let class_name = parts.next().unwrap_or("").to_string();

    Ok(Some((instance_name, class_name)))
}

fn hash_selection_data(data: &SelectionData) -> Result<u64> {
    let data_bin = bincode::encode_to_vec(data, BINCODE_CONFIG)?;
    let hash = ahash::RandomState::with_seed(HASH_SEED).hash_one(&data_bin);

    Ok(hash)
}

// Dumb algorithm here is fine I guess
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn trim_unicode_utf8(bytes: &[u8]) -> Result<&[u8]> {
    let s = str::from_utf8(bytes)?;
    let trimmed = s.trim();

    let start = trimmed.as_ptr() as usize - bytes.as_ptr() as usize;
    let end = start + trimmed.len();

    Ok(&bytes[start..end])
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
