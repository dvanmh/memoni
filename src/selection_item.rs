use std::{
    borrow::Cow,
    cell::Cell,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use bincode::{Decode, Encode};
use log::debug;
use self_cell::self_cell;

use crate::utils::{is_plaintext_mime, percent_decode_lossy, utf16le_to_string};

#[derive(Debug)]
pub struct SelectionTextData<'a> {
    pub plain: Option<Cow<'a, str>>,
    pub moz_url: Option<MozUrl>,
    pub files: Option<ActedOnUris<'a>>,
    pub all_raw: BTreeMap<&'a str, Cow<'a, str>>,
}

#[derive(Debug)]
pub struct MozUrl {
    pub src: String,
    pub alt: String,
}

#[derive(Debug)]
pub struct ActedOnUris<'a> {
    pub action: Cow<'a, str>,
    pub uris: Vec<Uri<'a>>,
}

#[derive(Debug)]
pub struct Uri<'a> {
    pub display: Cow<'a, str>,
    pub file_path: Option<Cow<'a, Path>>,
}

pub type SelectionData = BTreeMap<String, Vec<u8>>;

#[derive(Debug)]
struct SelectionItemData {
    id: u64,
    data: SelectionData,
    pinned: Cell<bool>,
}

self_cell!(
    pub struct SelectionItem {
        owner: SelectionItemData,

        #[covariant]
        dependent: SelectionTextData,
    }

    impl {Debug}
);

impl SelectionItem {
    pub fn create(id: u64, data: SelectionData) -> Self {
        Self::new(
            SelectionItemData {
                id,
                data,
                pinned: Cell::new(false),
            },
            |item_data| extract_text_from_data(item_data.id, &item_data.data),
        )
    }

    pub fn id(&self) -> u64 {
        self.borrow_owner().id
    }

    pub fn data(&self) -> &SelectionData {
        &self.borrow_owner().data
    }

    pub fn is_pinned(&self) -> bool {
        self.borrow_owner().pinned.get()
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.borrow_owner().pinned.set(pinned)
    }

    pub fn text_data(&self) -> &SelectionTextData<'_> {
        self.borrow_dependent()
    }
}

fn extract_text_from_data<'a>(id: u64, sel_data: &'a SelectionData) -> SelectionTextData<'a> {
    let mut all_raw: BTreeMap<&'a str, Cow<'a, str>> = BTreeMap::new();

    let mut text_content = None;
    let mut moz_url = None;
    let mut copied_files = None;
    for (mime, data) in sel_data {
        if is_plaintext_mime(mime) {
            text_content = Some(String::from_utf8_lossy(data));
        } else if mime == "text/x-moz-url" {
            // Firefox encodes data with UTF-16
            // https://stackoverflow.com/a/51581772
            let data = utf16le_to_string(data);

            moz_url = Some(
                data.split_once('\n')
                    .map(|(s, a)| MozUrl {
                        src: s.to_string(),
                        alt: a.to_string(),
                    })
                    .unwrap_or(MozUrl {
                        src: data.clone(),
                        alt: "".to_string(),
                    }),
            );

            all_raw.insert(mime, Cow::Owned(data));
        } else if mime == "x-special/gnome-copied-files" {
            let home = dirs::home_dir();

            let mut iter = data
                .strip_suffix(b"\n")
                .unwrap_or(data)
                .split(|&b| b == b'\n');

            let action = iter
                .next()
                .map(|l| String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)))
                .unwrap_or(Cow::Borrowed(""));
            copied_files = Some(ActedOnUris {
                action,
                uris: iter
                    .map(|line| {
                        let line =
                            String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line));
                        match strip_prefix_cow(line.clone(), "file://") {
                            Ok(file) => {
                                let file = percent_decode_lossy(file);
                                let (path, display) = match file {
                                    Cow::Borrowed(f) => {
                                        let p = Path::new(f);
                                        (Cow::Borrowed(p), format_path_display(p, &home))
                                    }
                                    Cow::Owned(f) => {
                                        let p = PathBuf::from(f);
                                        let d = format_path_display(&p, &home).into_owned();
                                        (Cow::Owned(p), Cow::Owned(d))
                                    }
                                };

                                Uri {
                                    display,
                                    file_path: Some(path),
                                }
                            }
                            Err(unknown) => Uri {
                                display: unknown,
                                file_path: None,
                            },
                        }
                    })
                    .collect(),
            });

            all_raw.insert(mime, String::from_utf8_lossy(data));
        } else if mime.starts_with("text/") {
            match str::from_utf8(data) {
                Ok(text) => {
                    all_raw.insert(mime, Cow::Borrowed(text));
                }
                Err(err) => {
                    debug!(
                        "error when UTF8-decoding data of {id} with mime {mime}, excluding from SelectionTextData: {err}"
                    );
                }
            };
        }
    }

    SelectionTextData {
        plain: text_content,
        moz_url,
        files: copied_files,
        all_raw,
    }
}

impl Encode for SelectionItem {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        self.borrow_owner().id.encode(encoder)?;
        self.borrow_owner().data.encode(encoder)?;
        Ok(())
    }
}

impl<Context> Decode<Context> for SelectionItem {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        let id: u64 = bincode::Decode::decode(decoder)?;
        let data: SelectionData = bincode::Decode::decode(decoder)?;
        Ok(Self::create(id, data))
    }
}

fn strip_prefix_cow<'a>(s: Cow<'a, str>, prefix: &str) -> Result<Cow<'a, str>, Cow<'a, str>> {
    match s {
        Cow::Borrowed(s) => match s.strip_prefix(prefix) {
            Some(stripped) => Ok(Cow::Borrowed(stripped)),
            None => Err(Cow::Borrowed(s)),
        },
        Cow::Owned(mut s) => {
            if s.starts_with(prefix) {
                s.drain(0..prefix.len());
                Ok(Cow::Owned(s))
            } else {
                Err(Cow::Owned(s))
            }
        }
    }
}

fn format_path_display<'a>(path: &'a Path, home: &Option<PathBuf>) -> Cow<'a, str> {
    let tilded: Cow<str> = match home {
        Some(home) => match path.strip_prefix(home) {
            Ok(rest) if rest.as_os_str().is_empty() => Cow::Borrowed("~"),
            Ok(rest) => Cow::Owned(format!("~/{}", rest.to_str().unwrap())),
            Err(_) => Cow::Borrowed(path.to_str().unwrap()),
        },
        None => Cow::Borrowed(path.to_str().unwrap()),
    };

    if path.is_dir() && !tilded.ends_with('/') {
        Cow::Owned(format!("{tilded}/"))
    } else {
        tilded
    }
}
