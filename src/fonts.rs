use std::sync::Arc;

use egui::{
    FallbackRequest, FontData, FontInsert, FontPriority, FontProvider, FontTweak, InsertFontFamily,
};
use fontique::{
    Attributes, Collection, CollectionOptions, GenericFamily, QueryFamily, QueryStatus, SourceCache,
};
use log::{debug, info, warn};
use skrifa::{FontRef, MetadataProvider as _};

const NOTO_SANS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/Noto_Sans/NotoSans-Regular.ttf"
));
const NOTO_MATH: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/Noto_Sans_Math/NotoSansMath-Regular.ttf"
));

pub const CONFIG_FONT_PREFIX: &str = "config:";
pub const NOTO_SANS_NAME: &str = "memoni:NotoSans-Regular";
pub const NOTO_MATH_NAME: &str = "memoni:NotoSansMath-Regular";

fn generic_family(family: &str) -> Option<GenericFamily> {
    Some(match family.trim().to_ascii_lowercase().as_str() {
        "sans-serif" | "sans serif" | "sans" => GenericFamily::SansSerif,
        "serif" => GenericFamily::Serif,
        "monospace" | "mono" => GenericFamily::Monospace,
        "emoji" => GenericFamily::Emoji,
        "system-ui" | "system ui" => GenericFamily::SystemUi,
        "cursive" => GenericFamily::Cursive,
        "fantasy" => GenericFamily::Fantasy,
        "math" => GenericFamily::Math,
        _ => return None,
    })
}

pub fn find_families(family_names: &[String]) -> Vec<(String, usize, ResolvedFont)> {
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let mut source_cache = SourceCache::new(Default::default());

    let mut found_families = Vec::new();
    for (i, family) in family_names.iter().enumerate() {
        match resolve(&mut collection, &mut source_cache, family) {
            Some(font) => {
                debug!("found font family '{family}'");
                found_families.push((CONFIG_FONT_PREFIX.to_owned() + family, i, font));
            }
            None => warn!("font family '{family}' not found"),
        }
    }

    if found_families.is_empty()
        && let Some(font) = resolve(&mut collection, &mut source_cache, "sans-serif")
    {
        info!("no configured font families resolved, using system sans-serif fonts");
        found_families.push(("memoni:system-sans-serif".to_owned(), 0, font));
    }

    found_families
}

fn resolve(
    collection: &mut Collection,
    source_cache: &mut SourceCache,
    family: &str,
) -> Option<ResolvedFont> {
    let query_family = match generic_family(family) {
        Some(generic) => QueryFamily::Generic(generic),
        None => QueryFamily::Named(family),
    };

    let mut query = collection.query(source_cache);
    query.set_families([query_family]);
    query.set_attributes(Attributes::default());

    let mut found = None;
    query.matches_with(|font| {
        if let Some(charmap) = font.charmap()
            && charmap.map(' ').is_some()
        {
            found = Some(ResolvedFont {
                data: font.blob.clone().into_raw_parts().0,
                index: font.index,
            });
            QueryStatus::Stop
        } else {
            QueryStatus::Continue
        }
    });

    found
}

pub struct ResolvedFont {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    index: u32,
}

impl ResolvedFont {
    pub fn font_data(&self, y_offset_factor: f32) -> FontData {
        FontData::from_blob(self.data.clone(), self.index).tweak(FontTweak {
            y_offset_factor,
            ..Default::default()
        })
    }
}

pub struct BuiltinFontProvider;

impl FontProvider for BuiltinFontProvider {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let c = request.cluster.chars().next()?;
        let candidates = [(NOTO_SANS_NAME, NOTO_SANS), (NOTO_MATH_NAME, NOTO_MATH)];
        let (name, bytes) = candidates.into_iter().find(|(_, bytes)| {
            BuiltinFont {
                data: Arc::new(*bytes),
                index: 0,
            }
            .covers_char(c)
        })?;
        Some(FontInsert {
            name: name.to_owned(),
            data: FontData::from_blob(Arc::new(bytes), 0),
            families: vec![InsertFontFamily {
                family: request.family.clone(),
                priority: FontPriority::Lowest,
            }],
        })
    }
}

pub struct BuiltinFont {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    index: u32,
}

impl BuiltinFont {
    fn covers_char(&self, c: char) -> bool {
        FontRef::from_index(self.data.as_ref().as_ref(), self.index)
            .is_ok_and(|font| font.charmap().map(c).is_some())
    }
}
