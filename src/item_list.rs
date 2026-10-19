use anyhow::{Result, bail};
use bincode::{Decode, Encode};

use crate::{
    ordered_hash_map::{OrderedHashMap, OrderedHashMapView},
    selection_item::{SelectionData, SelectionItem},
};

#[derive(Debug, Encode)]
pub struct ItemList {
    items: OrderedHashMap<u64, SelectionItem>,
    pinned_count: usize,
}

impl Default for ItemList {
    fn default() -> Self {
        ItemList {
            items: OrderedHashMap::new(),
            pinned_count: 0,
        }
    }
}

impl ItemList {
    pub fn new(mut items: OrderedHashMap<u64, SelectionItem>, pinned_count: usize) -> Self {
        for (_, item) in items.iter_mut().take(pinned_count) {
            item.set_pinned(true);
        }

        ItemList {
            items,
            pinned_count,
        }
    }

    pub fn get(&self, item_id: u64) -> Option<&SelectionItem> {
        self.items.get(&item_id)
    }

    pub fn contains(&self, item_id: u64) -> bool {
        self.items.contains_key(&item_id)
    }

    pub fn iter(&self) -> <&OrderedHashMap<u64, SelectionItem> as IntoIterator>::IntoIter {
        self.items.iter()
    }

    pub fn items(&self) -> &OrderedHashMap<u64, SelectionItem> {
        &self.items
    }

    pub fn to_view(&self) -> OrderedHashMapView<'_, u64, SelectionItem> {
        self.items.to_view()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn first_unpinned_id(&self) -> u64 {
        self.items
            .get_by_index(self.pinned_count)
            .map(|(id, _)| *id)
            .unwrap_or(0)
    }

    pub fn is_pinned(&self, item_id: u64) -> bool {
        self.get(item_id).is_some_and(|item| item.is_pinned())
    }

    pub fn toggle_pin(&mut self, item_id: u64) -> Result<bool> {
        let Some(mut item) = self.items.remove(&item_id) else {
            bail!("item not found: {item_id}");
        };
        let is_pinned = item.is_pinned();

        item.set_pinned(!is_pinned);
        if is_pinned {
            self.pinned_count -= 1;
            self.items.insert(self.pinned_count, item_id, item);
            Ok(false)
        } else {
            self.pinned_count += 1;
            self.items.push_front(item_id, item);
            Ok(true)
        }
    }

    pub fn pop_front(&mut self) -> Option<SelectionItem> {
        self.items.pop_front().map(|(_, item)| item)
    }

    pub fn remove(&mut self, item_id: u64) -> Option<SelectionItem> {
        self.items.remove(&item_id)
    }

    pub fn insert_after_pinned(&mut self, item_id: u64, data: SelectionData) {
        self.items.insert(
            self.pinned_count,
            item_id,
            SelectionItem::create(item_id, data),
        );
    }

    pub fn move_after_pinned_if_unpinned(&mut self, item_id: u64) {
        let Some(index) = self.items.iter().position(|(&id, _)| id == item_id) else {
            return;
        };
        if index >= self.pinned_count {
            let item = self.items.remove(&item_id).unwrap();
            self.items.insert(self.pinned_count, item_id, item);
        }
    }

    pub fn split_off(&mut self, limit: usize) -> impl Iterator<Item = SelectionItem> + '_ {
        self.items
            .split_off(limit)
            .into_iter()
            .map(|(_, item)| item)
    }
}

impl<Context> Decode<Context> for ItemList {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        let items: OrderedHashMap<u64, SelectionItem> = Decode::decode(decoder)?;
        let pinned_count: usize = Decode::decode(decoder)?;
        Ok(ItemList::new(items, pinned_count))
    }
}
