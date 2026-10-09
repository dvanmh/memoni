use log::debug;
use regex::Regex;

use crate::{
    config::{Color, SearchModeColor},
    ordered_hash_map::OrderedHashMap,
    selection_item::{SelectionItem, TextDataTag},
};

const TIER_PRIMARY: f32 = 1.5;
const TIER_SECONDARY: f32 = 1.25;
const TIER_RAW: f32 = 1.0;

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
    prev_mode: SearchMode,
}

pub struct SearchState {
    pub mode: SearchMode,
    pub invalid_regex: bool,
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
            prev_mode: SearchMode::Plain,
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

    pub fn mode_changed(&mut self) -> bool {
        if self.state.mode != self.prev_mode {
            self.prev_mode = self.state.mode;
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

        let mut matched: Vec<(u64, Vec<FieldMatch<'_>>)> = Vec::new();
        let mut fields: Vec<FieldMatch<'_>> = Vec::new();

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

                let mut prev_idx = None;
                for m in matches.into_iter().map(Some).chain(std::iter::once(None)) {
                    if let Some(prev_idx) = prev_idx
                        && m.as_ref()
                            .is_none_or(|m| owner[m.index as usize].0 != prev_idx)
                    {
                        matched.push((
                            *items.get_by_index(prev_idx).unwrap().0,
                            std::mem::take(&mut fields),
                        ));
                    }

                    if let Some(m) = m {
                        let (idx, tag) = &owner[m.index as usize];

                        // frizbee returns match indices in reverse order
                        let mut indices = m.indices;
                        indices.reverse();

                        fields.push(FieldMatch {
                            tag: tag.clone(),
                            base_score: f32::from(m.score),
                            indices,
                        });

                        prev_idx = Some(*idx);
                    }
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
                    for (s, tag) in item.text_data().flatten() {
                        let indices = query_regex
                            .find_iter(s)
                            .flat_map(|m| (m.start() as u32)..(m.end() as u32))
                            .collect::<Vec<_>>();

                        if !indices.is_empty() {
                            fields.push(FieldMatch {
                                tag,
                                base_score: 1.0,
                                indices,
                            });
                        }
                    }

                    if !fields.is_empty() {
                        matched.push((id, std::mem::take(&mut fields)));
                    }
                }
            }
        }

        let mut items_with_score = matched
            .into_iter()
            .flat_map(|(id, fields)| create_item_with_score(fields).map(|(m, s)| (id, m, s)))
            .collect::<Vec<_>>();
        items_with_score.sort_by(|(_, _, sa), (_, _, sb)| sb.total_cmp(sa));

        for (id, m, _) in items_with_score {
            self.visible_ids.push(id);
            self.matches.push(m);
        }
    }

    pub fn remove_id(&mut self, id: u64) {
        if let Some(index) = self.visible_ids.iter().position(|&vid| vid == id) {
            self.visible_ids.remove(index);
            self.matches.remove(index);
        }
    }
}

fn create_item_with_score(fields: Vec<FieldMatch<'_>>) -> Option<(SearchMatch, f32)> {
    if fields.is_empty() {
        return None;
    }

    let mut search_match = SearchMatch::default();
    let mut best_raw_base = 0f32;
    let mut best_score = 0f32;

    for field in fields {
        best_score = best_score.max(field.base_score * tier_weight(&field.tag));

        if matches!(&field.tag, TextDataTag::Raw { .. }) {
            if field.base_score >= best_raw_base {
                search_match.add(field.indices, &field.tag);
            }
            best_raw_base = best_raw_base.max(field.base_score);
        } else {
            search_match.add(field.indices, &field.tag);
        }
    }

    Some((search_match, best_score))
}

fn tier_weight(tag: &TextDataTag<'_>) -> f32 {
    match tag {
        TextDataTag::Plain | TextDataTag::FileUri(_) | TextDataTag::ImageAlt => TIER_PRIMARY,
        TextDataTag::ImageSrc | TextDataTag::FileAction => TIER_SECONDARY,
        TextDataTag::Raw { .. } => TIER_RAW,
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

struct FieldMatch<'a> {
    tag: TextDataTag<'a>,
    base_score: f32,
    indices: SearchMatchedBytes,
}
