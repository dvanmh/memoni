use std::cmp;

use log::debug;
use regex::Regex;

use crate::{
    config::{Color, SearchModeColor},
    ordered_hash_map::OrderedHashMap,
    selection_item::{SelectionItem, TextDataTag},
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
    pub matches: Vec<SearchMatch>,
    pub state: SearchState,
    prev_query: String,
}

impl Search {
    pub fn new() -> Self {
        Search {
            query: String::new(),
            visible_ids: vec![],
            matches: vec![],
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
        self.matches.clear();
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
        self.matches.clear();

        if self.query.is_empty() {
            self.visible_ids.clear();
            self.visible_ids.extend(items.iter().map(|(id, _)| *id));
            return;
        }

        match self.state.mode {
            SearchMode::Plain | SearchMode::Fuzzy => {
                self.visible_ids.clear();

                let mut haystacks: Vec<&str> = Vec::with_capacity(items.len() * 3);
                let mut owner: Vec<(usize, TextDataTag)> = Vec::with_capacity(items.len() * 3);
                for (outer_idx, (_, item)) in items.iter().enumerate() {
                    for (s, tag) in item.text_data().flatten() {
                        haystacks.push(s.as_ref());
                        owner.push((outer_idx, tag));
                    }
                }

                let mut matcher = match self.state.mode {
                    SearchMode::Plain => frizbee::Matcher::new(
                        &self.query,
                        &frizbee::Config {
                            matching: frizbee::Matching::Substring,
                            sort: frizbee::SortStrategy::IndexAsc,
                            ..frizbee::Config::default()
                        },
                    ),
                    SearchMode::Fuzzy => frizbee::Matcher::from_query(
                        &self.query,
                        &frizbee::Config {
                            sort: frizbee::SortStrategy::IndexAsc,
                            ..frizbee::Config::default()
                        },
                    ),
                    SearchMode::Regex => unreachable!(),
                };
                let matches = matcher.match_list_indices(&haystacks);
                if matches.is_empty() {
                    return;
                }

                let mut items_with_score = Vec::with_capacity(matches.len() / 2);
                let mut prev_idx = None;
                let mut prev_best_raw_score = 0u16;
                let mut search_match = SearchMatch::default();
                let mut best_score = 0u16;
                for m in matches.into_iter().map(Some).chain(std::iter::once(None)) {
                    if let Some(prev_idx) = prev_idx
                        && m.as_ref()
                            .is_none_or(|m| owner[m.index as usize].0 != prev_idx)
                    {
                        items_with_score.push((
                            *items.get_by_index(prev_idx).unwrap().0,
                            std::mem::take(&mut search_match),
                            best_score,
                        ));
                        prev_best_raw_score = 0;
                        best_score = 0;
                    }

                    if let Some(m) = m {
                        let (idx, tag) = &owner[m.index as usize];

                        let is_raw = matches!(tag, TextDataTag::Raw { .. });
                        if !(is_raw && m.score < prev_best_raw_score) {
                            // frizbee returns match indices in reverse order
                            let mut indices = m.indices;
                            indices.reverse();
                            search_match.add(indices, tag);
                        }
                        if is_raw {
                            prev_best_raw_score = prev_best_raw_score.max(m.score);
                        }

                        best_score = best_score.max(m.score);
                        prev_idx = Some(*idx);
                    }
                }

                items_with_score.sort_unstable_by_key(|i| cmp::Reverse(i.2));
                for (id, m, _) in items_with_score {
                    self.visible_ids.push(id);
                    self.matches.push(m);
                }
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

                for (&id, item) in items {
                    let mut matched = false;
                    let mut search_match = SearchMatch::default();
                    for (s, tag) in item.text_data().flatten() {
                        let match_indices = query_regex
                            .find_iter(s)
                            .flat_map(|m| (m.start() as u32)..(m.end() as u32))
                            .collect::<Vec<_>>();

                        if !match_indices.is_empty() {
                            matched = true;
                            search_match.add(match_indices, &tag);
                        }
                    }

                    if matched {
                        self.visible_ids.push(id);
                        self.matches.push(search_match);
                    }
                }
            }
        }
    }

    pub fn remove_id(&mut self, id: u64) {
        if let Some(index) = self.visible_ids.iter().position(|&vid| vid == id) {
            self.visible_ids.remove(index);
            self.matches.remove(index);
        }
    }
}

impl Default for Search {
    fn default() -> Self {
        Self::new()
    }
}

/// Matched bytes in order
pub type SearchMatchedBytes = Vec<u32>;

#[derive(Debug, Default)]
pub struct SearchMatch {
    pub plain: SearchMatchedBytes,
    pub image_src: SearchMatchedBytes,
    pub image_alt: SearchMatchedBytes,
    pub file_action: SearchMatchedBytes,
    pub file_uris: Vec<SearchMatchedBytes>,
    // TODO: display this
    pub best_raw: Option<(String, SearchMatchedBytes)>,
}

impl SearchMatch {
    pub fn add(&mut self, match_indices: Vec<u32>, tag: &TextDataTag<'_>) {
        match tag {
            TextDataTag::Plain => self.plain = match_indices,
            TextDataTag::ImageSrc => self.image_src = match_indices,
            TextDataTag::ImageAlt => self.image_alt = match_indices,
            TextDataTag::FileAction => self.file_action = match_indices,
            TextDataTag::FileUri(i) => {
                if *i >= self.file_uris.len() {
                    self.file_uris.resize_with(i + 1, Vec::new);
                }
                self.file_uris[*i] = match_indices;
            }
            TextDataTag::Raw { mime } => self.best_raw = Some((mime.to_string(), match_indices)),
        }
    }
}

pub struct SearchState {
    pub mode: SearchMode,
    pub invalid_regex: bool,
}
