use crate::{ordered_hash_map::OrderedHashMap, selection_item::SelectionItem};

pub struct Search {
    pub query: String,
    pub visible_ids: Vec<u64>,
    prev_query: String,
}

impl Search {
    pub fn new() -> Self {
        Search {
            query: String::new(),
            prev_query: String::new(),
            visible_ids: vec![],
        }
    }

    pub fn reset(&mut self) {
        self.query.clear();
        self.prev_query.clear();
        self.visible_ids.clear();
    }

    pub fn query_changed(&mut self) -> bool {
        if self.query != self.prev_query {
            self.prev_query = self.query.clone();
            true
        } else {
            false
        }
    }

    pub fn refresh(&mut self, items: &OrderedHashMap<u64, SelectionItem>) {
        self.visible_ids.clear();
        if self.query.is_empty() {
            self.visible_ids.extend(items.iter().map(|(id, _)| *id));
        } else {
            self.visible_ids.extend(
                items
                    .iter()
                    .filter(|(_, item)| searchable_strings(item).any(|s| s.contains(&self.query)))
                    .map(|(id, _)| *id),
            );
        }
    }
}

impl Default for Search {
    fn default() -> Self {
        Self::new()
    }
}

fn searchable_strings(item: &SelectionItem) -> impl Iterator<Item = &str> {
    let data = item.text_data();

    let plain = data.plain.iter().map(|c| c.as_ref());
    let moz = data
        .moz_url
        .iter()
        .flat_map(|m| [m.src.as_str(), m.alt.as_str()]);
    let files = data.files.iter().flat_map(|f| {
        f.uris
            .iter()
            .map(|u| u.display.as_ref())
            .chain(std::iter::once(f.action.as_ref()))
    });
    let raw = data.all_raw.values().map(|c| c.as_ref());

    plain.chain(moz).chain(files).chain(raw)
}
