use anyhow::{Result, anyhow};
use bincode::{Decode, Encode};
use log::{debug, error, info};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{Read, Write as _},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use crate::{
    item_list::ItemList, ordered_hash_map::OrderedHashMap, selection::SelectionType,
    selection_item::SelectionItem,
};

const BINCODE_CONFIG: bincode::config::Configuration = bincode::config::standard();
const BINARY_VERSION: u32 = 3;

struct SharedState {
    pending: Mutex<Option<Vec<u8>>>,
    condvar: Condvar,
}

pub struct Persistence {
    file_path: PathBuf,
    shared: Arc<SharedState>,
}

impl Persistence {
    pub fn new(selection_type: SelectionType, display_id: &Option<String>) -> Result<Self> {
        let xdg_data_home = dirs::data_dir()
            .ok_or_else(|| anyhow!("data directory not found"))?
            .join("memoni");
        fs::create_dir_all(&xdg_data_home)?;

        let file_name = if let Some(id) = display_id {
            format!(
                "{}_{}_selections",
                selection_type.to_string().to_lowercase(),
                id
            )
        } else {
            format!("{}_selections", selection_type.to_string().to_lowercase())
        };
        let file_path = xdg_data_home.join(file_name);
        let temp_file_path = file_path.with_extension("tmp");

        let shared = Arc::new(SharedState {
            pending: Mutex::new(None),
            condvar: Condvar::new(),
        });
        let shared_clone = shared.clone();
        let file_path_clone = file_path.clone();
        thread::spawn(move || {
            loop {
                let data = {
                    let mut p = shared_clone.pending.lock().unwrap();
                    while p.is_none() {
                        p = shared_clone.condvar.wait(p).unwrap();
                    }
                    p.take()
                };
                if let Some(serialized_data) = data
                    && let Err(e) =
                        write_to_disk(&file_path_clone, &temp_file_path, &serialized_data)
                {
                    error!("failed to save selection items in background: {e}");
                }
            }
        });

        Ok(Persistence { file_path, shared })
    }

    pub fn save_selection_data(&self, items: &ItemList, bookmarks: &ItemList) -> Result<()> {
        info!("saving selection items to {:?}", self.file_path);

        let serialized_data = bincode::encode_to_vec((items, bookmarks), BINCODE_CONFIG)?;
        let mut p = self.shared.pending.lock().unwrap();
        *p = Some(serialized_data);
        drop(p);
        self.shared.condvar.notify_one();

        Ok(())
    }

    pub fn load_selection_data(&self) -> Result<(ItemList, ItemList)> {
        if !self.file_path.exists() {
            info!("no persisted selection items file presented, skip loading");
            return Ok((ItemList::default(), ItemList::default()));
        }

        info!("loading selection items from {:?}", self.file_path);
        let mut file = File::open(&self.file_path)?;

        let mut version_buf = [0u8; 4];
        file.read_exact(&mut version_buf)?;
        let version = u32::from_le_bytes(version_buf);

        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        let items: Result<(ItemList, ItemList)> = match version {
            3 => bincode::decode_from_slice(&data, BINCODE_CONFIG)
                .map(|(data, _)| data)
                .map_err(Into::into),
            2 => decode_version_2(&data),
            // version 1 does not have version field unfortunately
            _ => Err(anyhow!("invalid binary version")),
        };

        let items = (match items {
            Ok(items) => Ok(items),
            Err(err) => {
                debug!("decoding failed, trying to decode using version 1 format");
                data.splice(0..0, version_buf);
                decode_version_1(&data).map_err(|ver1_err| {
                    debug!("decoding using version 1 format failed: {ver1_err}");
                    err
                })
            }
        })?;

        info!(
            "{} items and {} bookmarks loaded",
            items.0.len(),
            items.1.len()
        );
        Ok(items)
    }
}

fn write_to_disk(
    file_path: &PathBuf,
    temp_file_path: &PathBuf,
    serialized_data: &[u8],
) -> Result<()> {
    const CHUNK_SIZE: usize = 64 * 1024;

    let mut f = File::create(temp_file_path)?;
    f.write_all(&BINARY_VERSION.to_le_bytes())?;

    for chunk in serialized_data.chunks(CHUNK_SIZE) {
        f.write_all(chunk)?;
    }

    f.sync_all()?;
    fs::rename(temp_file_path, file_path)?;

    debug!("saving selection items in background completed");
    Ok(())
}

fn decode_version_2(data: &[u8]) -> Result<(ItemList, ItemList)> {
    let old_items: (OrderedHashMap<u64, SelectionItem>, SelectionMetadataV2) =
        bincode::decode_from_slice(data, BINCODE_CONFIG)?.0;

    Ok((
        ItemList::new(old_items.0, old_items.1.pinned_count),
        ItemList::default(),
    ))
}

fn decode_version_1(data: &[u8]) -> Result<(ItemList, ItemList)> {
    let old_items: VecDeque<SelectionItem> = bincode::decode_from_slice(data, BINCODE_CONFIG)?.0;
    let mut new_items = OrderedHashMap::new();
    for item in old_items {
        new_items.push_back(item.id(), item);
    }

    Ok((ItemList::new(new_items, 0), ItemList::default()))
}

#[derive(Debug, Default, Encode, Decode)]
struct SelectionMetadataV2 {
    pinned_count: usize,
}
