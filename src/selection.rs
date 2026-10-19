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

use anyhow::{Context as _, Result};
use log::{debug, error, info, trace, warn};
use x11rb::protocol::{
    Event,
    xfixes::{SelectionEvent, SelectionEventMask},
    xproto::{ConnectionExt as _, *},
};
use x11rb::{connection::Connection, connection::RequestConnection as _, protocol::xfixes};

use crate::{
    config::Config,
    item_list::ItemList,
    selection_item::{SelectionData, SelectionItem},
    transfer_window_pool::{TransferWindow, TransferWindowPool},
    utils::{image_mime_score, is_image_mime, is_plaintext_mime, plaintext_mime_score},
    x11_window::X11Window,
};

const HASH_SEED: usize = 0xfd9aadcf54cc0f35;
const BINCODE_CONFIG: bincode::config::Configuration = bincode::config::standard();
const OVERDUE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_INCR_SIZE: usize = 10 * 1024 * 1024;

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

#[derive(Debug, Default)]
pub enum EventHandlingResult {
    HistoryUpdated {
        added: Option<u64>,
        removed: Vec<SelectionItem>,
    },
    SelectionOwnerDied,
    #[default]
    None,
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
    pub history: ItemList,

    window: &'a X11Window<'a>,
    config: &'a Config,
    merge_consecutive_similar_items: bool,
    selection_atom: Atom,
    atoms: Atoms,
    request_tasks: HashMap<Window, Task<RequestTaskState, (Atom, Owner)>>,
    transfer_windows: TransferWindowPool<'a>,
    mime_atoms: RefCell<HashMap<String, Atom>>,
    prev_item_metadata: Option<(u32, Instant, bool)>,
}

impl<'a> Selection<'a> {
    pub fn new(
        history: ItemList,
        window: &'a X11Window,
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
            history,
            window,
            config,
            merge_consecutive_similar_items,
            selection_atom,
            atoms,
            request_tasks: HashMap::new(),
            transfer_windows: TransferWindowPool::new(conn, root, selection_type)?,
            mime_atoms: RefCell::new(HashMap::new()),
            prev_item_metadata: None,
        })
    }

    pub fn handle_event(&mut self, event: &Event) -> Result<EventHandlingResult> {
        let conn = &self.window.conn;
        let paste_window = self.window.win_id.get();
        let atoms = self.atoms;

        'blk: {
            match event {
                // Capture copied data
                Event::XfixesSelectionNotify(ev) => {
                    if ev.selection != self.selection_atom {
                        debug!(
                            "ignoring selection notification for other selection {:?}",
                            ev.selection
                        );
                        break 'blk;
                    }

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
                            info!(
                                "selection owner died ({:?}), handing over to fallback logic",
                                ev.subtype
                            );
                            return Ok(EventHandlingResult::SelectionOwnerDied);
                        }
                        _ => {}
                    }
                }
                Event::SelectionNotify(ev) => {
                    if ev.selection != self.selection_atom {
                        trace!(
                            "ignoring selection notification for other selection {:?}",
                            ev.selection
                        );
                        break 'blk;
                    }

                    let transfer_window = ev.requestor;
                    let Some(task) = self.request_tasks.get_mut(&transfer_window) else {
                        trace!(
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
                Event::PropertyNotify(ev) => {
                    if ev.window == self.window.screen.root {
                        trace!("ignoring root window property change: {:?}", ev.atom);
                        break 'blk;
                    }

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

                _ => {}
            }
        }

        self.purge_overdue_tasks();

        Ok(EventHandlingResult::None)
    }

    fn process_selection_data(
        &mut self,
        transfer_window: TransferWindow,
        value: Vec<u8>,
        mime_name: Option<String>,
        mime_atom: Atom,
        owner: Owner,
    ) -> Result<EventHandlingResult> {
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
            return Ok(EventHandlingResult::None);
        }

        self.transfer_windows.release(transfer_window);

        if data.is_empty() {
            warn!("dropping empty selection");
            return Ok(EventHandlingResult::None);
        }

        let prev_item = self.history.items().front();
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
            removed.push(self.history.pop_front().unwrap());
        }

        let (new_item_id, new_item, removed) =
            self.insert_data_item(mem::take(data), removed, owner)?;

        info!("selection transfer completed with new selection: {new_item_id}");
        Ok(EventHandlingResult::HistoryUpdated {
            added: new_item.map(|item| item.id()),
            removed,
        })
    }

    pub fn insert_text_item(
        &mut self,
        text: &str,
    ) -> Result<(Option<&SelectionItem>, Vec<SelectionItem>)> {
        info!("inserting highlighted search text into history");
        let data = SelectionData::from([(
            "text/plain;charset=utf-8".to_owned(),
            text.as_bytes().to_vec(),
        )]);
        let (_, new_item, removed) = self.insert_data_item(data, Vec::new(), 0)?;
        Ok((new_item, removed))
    }

    fn insert_data_item(
        &mut self,
        mut data: SelectionData,
        mut removed: Vec<SelectionItem>,
        owner: Owner,
    ) -> Result<(u64, Option<&SelectionItem>, Vec<SelectionItem>)> {
        let insert_item_id = hash_selection_data(&data)?;

        let mut is_previously_seen = false;
        let mut new_item = None;
        if self.history.contains(insert_item_id) {
            if self
                .history
                .get(insert_item_id)
                .is_some_and(|i| i.is_pinned())
            {
                debug!("selection is duplicated, old one is pinned, keeping old selection");
            } else {
                debug!("selection is duplicated, removing old one");
                let previous_seen_item = self.history.remove(insert_item_id).unwrap();
                self.history
                    .insert_after_pinned(insert_item_id, previous_seen_item.data().clone());
            }

            is_previously_seen = true;
        } else {
            self.history
                .insert_after_pinned(insert_item_id, mem::take(&mut data));

            if self.history.len() > self.config.item_limit {
                removed.extend(self.history.split_off(self.config.item_limit));
            };

            new_item = self.history.get(insert_item_id);
        }

        self.prev_item_metadata = Some((owner, Instant::now(), is_previously_seen));
        Ok((insert_item_id, new_item, removed))
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
    }
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

fn hash_selection_data(data: &SelectionData) -> Result<u64> {
    let data_bin = bincode::encode_to_vec(data, BINCODE_CONFIG)?;
    let hash = ahash::RandomState::with_seed(HASH_SEED).hash_one(&data_bin);

    Ok(hash)
}

// Dumb algorithm here is fine I guess
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
