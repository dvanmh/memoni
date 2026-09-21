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
    Fuzzy,
    Regex,
}

impl SearchMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Plain => Self::Fuzzy,
            Self::Fuzzy => Self::Regex,
            Self::Regex => Self::Plain,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::Fuzzy => "FUZZY",
            Self::Regex => "REGEX",
        }
    }

    pub fn color(&self, config: &SearchModeColor) -> Color {
        match self {
            Self::Plain => config.plain,
            Self::Fuzzy => config.fuzzy,
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
        if self.query.is_empty() {
            self.visible_ids.clear();
            self.visible_ids.extend(items.iter().map(|(id, _)| *id));
            return;
        }

        match self.state.mode {
            SearchMode::Plain | SearchMode::Fuzzy => {
                let mut best: Vec<Option<u16>> = vec![None; items.len()];

                let mut haystacks: Vec<&str> = vec![];
                let mut owner: Vec<usize> = vec![];
                for (outer_idx, (_, item)) in items.iter().enumerate() {
                    for s in searchable_strings(item) {
                        haystacks.push(s);
                        owner.push(outer_idx);
                    }
                }

                let mut matcher = match self.state.mode {
                    SearchMode::Plain => frizbee::Matcher::new(
                        &self.query,
                        &frizbee::Config {
                            matching: frizbee::Matching::Substring,
                            ..frizbee::Config::default()
                        },
                    ),
                    SearchMode::Fuzzy => {
                        frizbee::Matcher::from_query(&self.query, &frizbee::Config::default())
                    }
                    SearchMode::Regex => unreachable!(),
                };
                let matches = matcher.match_list(&haystacks);
                for m in &matches {
                    let outer = owner[m.index as usize];
                    let entry = &mut best[outer];
                    *entry = Some(entry.map_or(m.score, |s| s.max(m.score)));
                }

                let mut results = best
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| s.map(|s| (i, s)))
                    .collect::<Vec<_>>();
                results.sort_unstable_by_key(|&(_, s)| std::cmp::Reverse(s));

                self.visible_ids.clear();
                self.visible_ids.extend(
                    results
                        .iter()
                        .map(|(i, _)| items.get_by_index(*i).unwrap().0),
                );
            }
            SearchMode::Regex => {
                let query_regex = match Regex::new(&self.query) {
                    Ok(re) => {
                        self.state.invalid_regex = false;
                        re
                    }
                    Err(err) => {
                        debug!("invalid search regex query {:?}: {err}", self.query);
                        self.state.invalid_regex = true;
                        return;
                    }
                };

                self.visible_ids.clear();
                self.visible_ids.extend(
                    items
                        .iter()
                        .filter(|(_, item)| {
                            searchable_strings(item).any(|s| query_regex.is_match(s))
                        })
                        .map(|(id, _)| *id),
                );
            }
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
    let image_metadata = [
        data.image_metadata.src.as_deref(),
        data.image_metadata.alt.as_deref(),
    ]
    .into_iter()
    .flatten();
    let files = data.files.iter().flat_map(|f| {
        f.uris
            .iter()
            .map(|u| u.display.as_ref())
            .chain(std::iter::once(f.action.as_ref()))
    });
    let raw = data.all_raw.values().map(|c| c.as_ref());

    plain.chain(image_metadata).chain(files).chain(raw)
}
