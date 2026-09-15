use log::debug;
use regex::Regex;

use crate::{
    config::{Color, SearchModeColor},
    ordered_hash_map::OrderedHashMap,
    selection_item::SelectionItem,
};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SearchMode {
    Plain,
    Regex,
}

impl SearchMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Plain => Self::Regex,
            Self::Regex => Self::Plain,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::Regex => "REGEX",
        }
    }

    pub fn color(&self, config: &SearchModeColor) -> Color {
        match self {
            Self::Plain => config.plain,
            Self::Regex => config.regex,
        }
    }
}

pub struct Search {
    pub query: String,
    pub visible_ids: Vec<u64>,
    pub state: SearchState,
    prev_query: String,
}

impl Search {
    pub fn new() -> Self {
        Search {
            query: String::new(),
            visible_ids: vec![],
            state: SearchState {
                mode: SearchMode::Plain,
                invalid_regex: false,
            },
            prev_query: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.query.clear();
        self.prev_query.clear();
        self.state.mode = SearchMode::Plain;
        self.state.invalid_regex = false;
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
        let query_regex = if !self.query.is_empty() && self.state.mode == SearchMode::Regex {
            match Regex::new(&self.query) {
                Ok(re) => {
                    self.state.invalid_regex = false;
                    Some(re)
                }
                Err(err) => {
                    debug!("invalid search regex query {:?}: {err}", self.query);
                    self.state.invalid_regex = true;
                    return;
                }
            }
        } else {
            None
        };

        self.visible_ids.clear();

        if self.query.is_empty() {
            self.visible_ids.extend(items.iter().map(|(id, _)| *id));
        } else {
            self.visible_ids.extend(
                items
                    .iter()
                    .filter(|(_, item)| {
                        searchable_strings(item).any(|s| match self.state.mode {
                            SearchMode::Plain => s.contains(&self.query),
                            SearchMode::Regex => query_regex.as_ref().unwrap().is_match(s),
                        })
                    })
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

pub struct SearchState {
    pub mode: SearchMode,
    pub invalid_regex: bool,
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
