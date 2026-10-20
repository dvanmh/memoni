use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use egui::{
    FallbackRequest, FontData, FontDefinitions, FontFamily, FontInsert, FontPriority, FontProvider,
    FontTweak, InsertFontFamily,
};
use egui_system_font_provider::SystemFontProvider;
use fontique::{
    Attributes, Collection, CollectionOptions, GenericFamily, QueryFamily, QueryFont, QueryStatus,
    SourceCache, SourceCacheOptions,
};
use indexmap::IndexSet;
use log::{debug, warn};
use skrifa::{FontRef, GlyphId, MetadataProvider as _};

const NOTO_SANS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/Noto_Sans/NotoSans-Regular.ttf"
));
const NOTO_MATH: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/Noto_Sans_Math/NotoSansMath-Regular.ttf"
));

pub const NOTO_SANS_CONFIG_NAME: &str = "memoni:noto-sans";
pub const NOTO_MATH_CONFIG_NAME: &str = "memoni:noto-sans-math";
pub const SYSTEM_FALLBACK_CONFIG_NAME: &str = "system-fallback";

const CONFIG_FONT_PREFIX: &str = "config:";
const NOTO_SANS_NAME: &str = "memoni:NotoSans-Regular";
const NOTO_MATH_NAME: &str = "memoni:NotoSansMath-Regular";

pub struct FontPlan {
    pub definitions: FontDefinitions,
    pub providers: Vec<Arc<dyn FontProvider>>,
}

pub fn plan(family_names: &IndexSet<String>, y_offset_factors: &[f32]) -> Result<FontPlan> {
    if family_names.is_empty() {
        bail!("no family names to plan");
    }

    let fontique_state = FontiqueState::new();
    let system_font_provider = Arc::new(SystemFontProvider::new());

    let mut definitions = FontDefinitions::default();
    let mut family_font_names = vec![];
    let mut providers: Vec<Arc<dyn FontProvider>> = Vec::new();

    let mut as_provider = false;
    for (i, name) in family_names.iter().enumerate() {
        let entry = FontEntry::parse(name);
        let entry_name = entry.name();
        let y_offset_factor = y_offset_factors.get(i).copied().unwrap_or(0.0);
        let font_tweak = FontTweak {
            y_offset_factor,
            ..Default::default()
        };

        if matches!(entry, FontEntry::Generic(_) | FontEntry::SystemFallback) {
            as_provider = true;
        }

        if as_provider
            && family_font_names.is_empty()
            && let Some(font_insert) = entry.font_insert(&fontique_state, &system_font_provider)
        {
            debug!(
                "no concrete fonts available, installing '{}' as the first font provided by the dynamic provider chain",
                font_insert.name
            );
            definitions.font_data.insert(
                entry_name.clone(),
                Arc::new(font_insert.data.tweak(font_tweak.clone())),
            );
            family_font_names.push(entry_name.clone());
        }

        if as_provider {
            providers.push(match entry {
                FontEntry::SystemFallback => system_font_provider.clone(),
                FontEntry::Builtin(_) => Arc::new(StaticFontProvider {
                    insert_name: entry_name,
                    font_data: entry
                        .font_insert(&fontique_state, &system_font_provider)
                        .expect("FontEntry::Builtin always has font_insert")
                        .data,
                }),
                FontEntry::Named(name) => Arc::new(FamilyFontProvider {
                    spec: FamilySpec::Named(name),
                    fontique_state: fontique_state.clone(),
                    y_offset_factor,
                }),
                FontEntry::Generic(generic) => Arc::new(FamilyFontProvider {
                    spec: FamilySpec::Generic(generic),
                    fontique_state: fontique_state.clone(),
                    y_offset_factor,
                }),
            });
        } else {
            let Some(font_insert) = entry.font_insert(&fontique_state, &system_font_provider)
            else {
                warn!("font family '{name}' not found");
                continue;
            };

            definitions.font_data.insert(
                entry_name.clone(),
                Arc::new(font_insert.data.tweak(font_tweak)),
            );
            family_font_names.push(entry_name);
        }
    }

    if family_font_names.is_empty() {
        bail!("no configured font families resolved and the font provider chain found no fonts");
    }
    definitions
        .families
        .insert(FontFamily::Proportional, family_font_names);

    debug!(
        "font plan: {} installed, {} fallback providers",
        definitions.font_data.len(),
        providers.len()
    );

    Ok(FontPlan {
        definitions,
        providers,
    })
}

struct StaticFontProvider {
    insert_name: String,
    font_data: FontData,
}

impl FontProvider for StaticFontProvider {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let c = request.cluster.chars().next()?;
        if !self.covers_char(c) {
            return None;
        }

        Some(FontInsert {
            name: self.insert_name.clone(),
            data: self.font_data.clone(),
            families: vec![InsertFontFamily {
                family: request.family.clone(),
                priority: FontPriority::Lowest,
            }],
        })
    }
}

impl StaticFontProvider {
    fn covers_char(&self, c: char) -> bool {
        let font_ref = match FontRef::from_index(self.font_data.bytes(), self.font_data.index) {
            Ok(font_ref) => font_ref,
            Err(err) => {
                debug!(
                    "failed to create FontRef for {:?}: {err:?}",
                    self.insert_name
                );
                return false;
            }
        };

        font_ref
            .charmap()
            .map(c)
            .is_some_and(|glyph_id| glyph_id != GlyphId::NOTDEF)
    }
}

struct FamilyFontProvider {
    spec: FamilySpec,
    fontique_state: FontiqueState,
    y_offset_factor: f32,
}

impl FontProvider for FamilyFontProvider {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let c = request.cluster.chars().next()?;
        let font = self
            .fontique_state
            .query_font(self.spec.query_family(), |font| {
                can_render(font, c).then(|| font.clone())
            })?;

        Some(FontInsert {
            name: self.fontique_state.insert_name(&font),
            data: FontData::from_blob(font.blob.clone().into_raw_parts().0, font.index).tweak(
                FontTweak {
                    y_offset_factor: self.y_offset_factor,
                    ..Default::default()
                },
            ),
            families: vec![InsertFontFamily {
                family: request.family.clone(),
                priority: FontPriority::Lowest,
            }],
        })
    }
}

enum FontEntry {
    Builtin(BuiltinFont),
    Named(String),
    Generic(GenericFamily),
    SystemFallback,
}

impl FontEntry {
    fn parse(name: &str) -> Self {
        if name == SYSTEM_FALLBACK_CONFIG_NAME {
            return Self::SystemFallback;
        }
        if let Some(builtin) = BuiltinFont::parse(name) {
            return Self::Builtin(builtin);
        }
        if let Some(generic) = GenericFamily::parse(name) {
            return Self::Generic(generic);
        }
        Self::Named(name.to_owned())
    }

    fn name(&self) -> String {
        match self {
            Self::Builtin(builtin) => builtin.name().into(),
            Self::Named(name) => format!("{CONFIG_FONT_PREFIX}{name}"),
            Self::Generic(generic) => generic.to_string(),
            Self::SystemFallback => SYSTEM_FALLBACK_CONFIG_NAME.into(),
        }
    }

    fn font_insert(
        &self,
        fontique_state: &FontiqueState,
        system_font_provider: &SystemFontProvider,
    ) -> Option<FontInsert> {
        match self {
            FontEntry::Builtin(builtin) => Some(FontInsert {
                name: builtin.name().into(),
                data: FontData::from_static(builtin.bytes()),
                families: vec![],
            }),
            FontEntry::Named(name) => {
                let font = fontique_state
                    .query_font(QueryFamily::Named(name), |font| Some(font.clone()))?;
                Some(Self::query_font_to_font_insert(&font, fontique_state))
            }
            FontEntry::Generic(generic) => {
                let font = fontique_state
                    .query_font(QueryFamily::Generic(*generic), |font| Some(font.clone()))?;
                Some(Self::query_font_to_font_insert(&font, fontique_state))
            }
            FontEntry::SystemFallback => {
                // `SystemFontProvider` is opaque (the crate exposes only per-character lookups),
                // so it is probed for a font that covers a space
                system_font_provider.font_for(&FallbackRequest {
                    cluster: " ",
                    family: &FontFamily::Proportional,
                })
            }
        }
    }

    fn query_font_to_font_insert(font: &QueryFont, fontique_state: &FontiqueState) -> FontInsert {
        FontInsert {
            name: fontique_state.insert_name(font),
            data: FontData::from_blob(font.blob.clone().into_raw_parts().0, font.index),
            families: vec![],
        }
    }
}

#[derive(Clone)]
struct FontiqueState(Arc<Mutex<(Collection, SourceCache)>>);

impl FontiqueState {
    fn new() -> Self {
        Self(Arc::new(Mutex::new((
            Collection::new(CollectionOptions {
                shared: false,
                system_fonts: true,
            }),
            SourceCache::new(SourceCacheOptions { shared: false }),
        ))))
    }

    fn query_font<R>(
        &self,
        family: QueryFamily<'_>,
        mut match_font: impl FnMut(&QueryFont) -> Option<R>,
    ) -> Option<R> {
        let mut fonts = self.0.lock().unwrap();
        let (collection, source_cache) = &mut *fonts;

        let mut query = collection.query(source_cache);
        query.set_families([family]);
        query.set_attributes(Attributes::default());

        let mut found = None;
        query.matches_with(|font| match (match_font)(font) {
            Some(resolved) => {
                found = Some(resolved);
                QueryStatus::Stop
            }
            None => QueryStatus::Continue,
        });

        found
    }

    fn insert_name(&self, font: &QueryFont) -> String {
        let mut fonts = self.0.lock().unwrap();
        let (collection, _) = &mut *fonts;

        let family_name = collection.family_name(font.family.0).unwrap_or("unknown");
        format!("system:{family_name}:{}:{}", font.family.1, font.index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltinFont {
    Sans,
    Math,
}

impl BuiltinFont {
    fn parse(name: &str) -> Option<Self> {
        match name {
            NOTO_SANS_CONFIG_NAME => Some(Self::Sans),
            NOTO_MATH_CONFIG_NAME => Some(Self::Math),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Sans => NOTO_SANS_NAME,
            Self::Math => NOTO_MATH_NAME,
        }
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Sans => NOTO_SANS,
            Self::Math => NOTO_MATH,
        }
    }
}

enum FamilySpec {
    Generic(GenericFamily),
    Named(String),
}

impl FamilySpec {
    fn query_family(&self) -> QueryFamily<'_> {
        match self {
            Self::Generic(generic) => QueryFamily::Generic(*generic),
            Self::Named(name) => QueryFamily::Named(name),
        }
    }
}

/// Does the font have a glyph for the character that epaint can render?
///
/// That is an outline, a bitmap (`sbix`, `CBDT`), or a `COLR` glyph.
///
/// Copied verbatim from [egui_system_font_provider](https://github.com/emilk/egui/blob/db63b6d1765552b345491ae4ee5894de8440cd44/crates/egui_system_font_provider/src/lib.rs#L236)
fn can_render(font: &QueryFont, c: char) -> bool {
    use skrifa::MetadataProvider as _;

    let Some(glyph_id) = font.charmap().and_then(|charmap| charmap.map(c)) else {
        return false;
    };
    let glyph_id = skrifa::GlyphId::new(glyph_id);
    if glyph_id == skrifa::GlyphId::NOTDEF {
        return false;
    }
    let Ok(font_ref) = skrifa::FontRef::from_index(font.blob.as_ref(), font.index) else {
        return false;
    };
    font_ref.outline_glyphs().get(glyph_id).is_some()
        || font_ref.color_glyphs().get(glyph_id).is_some()
        || font_ref
            .bitmap_strikes()
            .glyph_for_size(skrifa::instance::Size::unscaled(), glyph_id)
            .is_some()
}
