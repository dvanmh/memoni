use core::fmt;
use std::{borrow::Cow, iter, sync::LazyLock};

use anyhow::{Context as _, Result, anyhow, bail};
use egui::{Key, Modifiers, PointerButton};
use itertools::{Either, Itertools as _};

use crate::AppMode;

pub struct KeymapSpec {
    pub mode: AppMode,
    pub name: &'static str,
    pub entries: &'static [KeymapEntry],
}

pub struct KeymapEntry {
    pub bindings: &'static [&'static str],
    pub description: &'static str,
    pub action: fn(usize, Source) -> Action,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Source {
    Key,
    Pointer,
}

impl KeymapEntry {
    pub const fn new(
        bindings: &'static [&'static str],
        description: &'static str,
        action: fn(usize, Source) -> Action,
    ) -> Self {
        Self {
            bindings,
            description,
            action,
        }
    }
}

macro_rules! plain {
    ($n:expr) => {
        |_, _| -> Action { $n }
    };
}

fn paste_action(modifier: PasteModifier, source: Source) -> Action {
    match source {
        Source::Key => Action::Key(KeyAction::Paste(modifier)),
        Source::Pointer => Action::Pointer(PointerAction::Paste(modifier)),
    }
}

#[rustfmt::skip]
pub static KEYMAP_SPECS: [KeymapSpec; 3] = [
    KeymapSpec {
        mode: AppMode::Normal,
        name: "Normal",
        entries: &[

            // Navigation
            KeymapEntry::new(
                &["↑", "k", "C-p", "S-Tab"],
                "Move to previous item",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::ItemUp))),
            ),
            KeymapEntry::new(
                &["↓", "j", "C-n", "Tab"],
                "Move to next item",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::ItemDown))),
            ),
            KeymapEntry::new(
                &["C-u"],
                "Scroll half page up",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::HalfUp))),
            ),
            KeymapEntry::new(
                &["C-d"],
                "Scroll half page down",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::HalfDown))),
            ),
            KeymapEntry::new(
                &["C-b"],
                "Scroll page up",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::PageUp))),
            ),
            KeymapEntry::new(
                &["C-f"],
                "Scroll page down",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::PageDown))),
            ),
            KeymapEntry::new(
                &["g g"],
                "Go to first item",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::ToTop))),
            ),
            KeymapEntry::new(
                &["S-g"],
                "Go to last item",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::ToBottom))),
            ),

            // Pasting
            KeymapEntry::new(
                &["Enter", "Space", "<pointer-1>"],
                "Paste item",
                |_, source| paste_action(PasteModifier::NONE, source),
            ),
            KeymapEntry::new(
                &["C-Enter", "C-Space", "C-<pointer-1>"],
                "Paste item and press Enter",
                |_, source| paste_action(PasteModifier::AND_ENTER, source),
            ),
            KeymapEntry::new(
                &["S-Enter", "S-Space", "S-<pointer-1>"],
                "Paste trimmed item",
                |_, source| paste_action(PasteModifier::TRIM, source),
            ),
            KeymapEntry::new(
                &["C-S-Enter", "C-S-Space", "C-S-<pointer-1>"],
                "Paste trimmed item and press Enter",
                |_, source| paste_action(PasteModifier::TRIM_AND_ENTER, source),
            ),
            KeymapEntry::new(
                &["1…9", "0"],
                "Quick paste item 1-10",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::NONE)),
            ),
            KeymapEntry::new(
                &["C-1…9", "C-0"],
                "Quick paste item 1-10 and press Enter",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::AND_ENTER)),
            ),
            KeymapEntry::new(
                &["S-1…9", "S-0"],
                "Quick paste trimmed item 1-10",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::TRIM)),
            ),
            KeymapEntry::new(
                &["C-S-1…9", "C-S-0"],
                "Quick paste trimmed item 1-10 and press Enter",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::TRIM_AND_ENTER)),
            ),

            // Other actions
            KeymapEntry::new(
                &["d d", "Delete"],
                "Remove item",
                plain!(Action::Key(KeyAction::Remove)),
            ),
            KeymapEntry::new(
                &["p"],
                "Toggle pin",
                plain!(Action::Key(KeyAction::Pin)),
            ),
            KeymapEntry::new(
                &["Esc", "q"],
                "Close window",
                plain!(Action::Key(KeyAction::Close)),
            ),
            KeymapEntry::new(
                &["/"],
                "Show search",
                plain!(Action::Key(KeyAction::ShowSearch)),
            ),
            KeymapEntry::new(
                &["S-/"],
                "Show help",
                plain!(Action::Key(KeyAction::ShowHelp)),
            ),
        ],
    },
    KeymapSpec {
        mode: AppMode::Search,
        name: "Search",
        entries: &[

            // Navigation
            KeymapEntry::new(
                &["↑", "C-p"],
                "Move to previous item",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::ItemUp))),
            ),
            KeymapEntry::new(
                &["↓", "C-n"],
                "Move to next item",
                plain!(Action::Key(KeyAction::Scroll(ScrollAction::ItemDown))),
            ),

            // Pasting
            KeymapEntry::new(
                &["Enter", "<pointer-1>"],
                "Paste item",
                |_, source| paste_action(PasteModifier::NONE, source),
            ),
            KeymapEntry::new(
                &["C-Enter", "C-<pointer-1>"],
                "Paste item and press Enter",
                |_, source| paste_action(PasteModifier::AND_ENTER, source),
            ),
            KeymapEntry::new(
                &["S-Enter", "S-<pointer-1>"],
                "Paste trimmed item",
                |_, source| paste_action(PasteModifier::TRIM, source),
            ),
            KeymapEntry::new(
                &["C-S-Enter", "C-S-<pointer-1>"],
                "Paste trimmed item and press Enter",
                |_, source| paste_action(PasteModifier::TRIM_AND_ENTER, source),
            ),
            KeymapEntry::new(
                &["M-1…9", "M-0"],
                "Quick paste item 1-10",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::NONE)),
            ),
            KeymapEntry::new(
                &["C-M-1…9", "C-M-0"],
                "Quick paste item 1-10 and press Enter",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::AND_ENTER)),
            ),
            KeymapEntry::new(
                &["S-M-1…9", "S-M-0"],
                "Quick paste trimmed item 1-10",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::TRIM)),
            ),
            KeymapEntry::new(
                &["C-S-M-1…9", "C-S-M-0"],
                "Quick paste trimmed item 1-10 and press Enter",
                |index, _| Action::Key(KeyAction::QuickPaste(index, PasteModifier::TRIM_AND_ENTER)),
            ),

            // Prompt editing
            KeymapEntry::new(
                &["←"],
                "Move caret one character left",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["→"],
                "Move caret one character right",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-←"],
                "Move caret one word left",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-→"],
                "Move caret one word right",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["S-←"],
                "Extend selection one character left",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["S-→"],
                "Extend selection one character right",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-S-←"],
                "Extend selection one word left",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-S-→"],
                "Extend selection one word right",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["Home"],
                "Move caret to start of line",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["End"],
                "Move caret to end of line",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["S-Home"],
                "Select to start of line",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["S-End"],
                "Select to end of line",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["Backspace"],
                "Delete character left",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["Delete"],
                "Delete character right",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-Backspace"],
                "Delete word left",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-Delete"],
                "Delete word right",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-z"],
                "Undo",
                plain!(Action::Passthrough),
            ),
            KeymapEntry::new(
                &["C-y"],
                "Redo",
                plain!(Action::Passthrough),
            ),

            // Other actions
            KeymapEntry::new(
                &["Esc"],
                "Exit search",
                plain!(Action::Key(KeyAction::Close)),
            ),
        ],
    },
    KeymapSpec {
        mode: AppMode::Help,
        name: "Help",
        entries: &[
            KeymapEntry::new(
                &["↑", "k", "C-p"],
                "Scroll up",
                plain!(Action::Key(KeyAction::SimpleScroll(SimpleScrollAction::Up))),
            ),
            KeymapEntry::new(
                &["↓", "j", "C-n"],
                "Scroll down",
                plain!(Action::Key(KeyAction::SimpleScroll(SimpleScrollAction::Down))),
            ),
            KeymapEntry::new(
                &["Esc", "q"],
                "Close help",
                plain!(Action::Key(KeyAction::Close)),
            ),
        ],
    },
];

#[derive(Debug)]
pub struct KeymapGroup {
    pub mode: AppMode,
    pub bindings: Vec<KeyBinding>,
}

#[derive(Debug)]
pub struct KeyBinding {
    pub keys: Vec<KeyChord>,
    pub action: Action,
}

pub static ACTION_KEYMAPS: LazyLock<Vec<KeymapGroup>> =
    LazyLock::new(|| compile(&KEYMAP_SPECS).expect("invalid keymap spec"));

fn compile(specs: &[KeymapSpec]) -> Result<Vec<KeymapGroup>> {
    let mut groups = vec![];
    for spec in specs {
        let mut bindings = vec![];
        for entry in spec.entries {
            bindings.append(&mut entry.compile()?);
        }

        groups.push(KeymapGroup {
            mode: spec.mode,
            bindings,
        })
    }

    Ok(groups)
}

impl KeymapEntry {
    fn compile(&self) -> Result<Vec<KeyBinding>> {
        self.bindings
            .iter()
            .flat_map(|binding| {
                match itertools::process_results(binding.split(" ").map(KeyChord::parse), |it| {
                    it.multi_cartesian_product()
                }) {
                    Ok(it) => Either::Left(it.enumerate().map(Ok)),
                    Err(e) => Either::Right(iter::once(Err(e))),
                }
            })
            .map_ok(|(i, chords)| {
                let source = match chords.last().map(|chord| chord.key) {
                    Some(KeyOrPointerButton::PointerButton(_)) => Source::Pointer,
                    _ => Source::Key,
                };
                KeyBinding {
                    keys: chords,
                    action: (self.action)(i, source),
                }
            })
            .collect::<Result<Vec<_>>>()
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct KeyChord {
    pub key: KeyOrPointerButton,
    pub mods: Modifiers,
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut write_part = |str| -> fmt::Result {
            if !first {
                write!(f, "-")?;
            }
            first = false;
            write!(f, "{str}")?;
            Ok(())
        };

        if self.mods.contains(Modifiers::CTRL) {
            write_part("C")?;
        }
        if self.mods.contains(Modifiers::ALT) {
            write_part("M")?;
        }
        if self.mods.contains(Modifiers::SHIFT) {
            write_part("S")?;
        }

        write_part(&self.key.name())?;

        Ok(())
    }
}

impl KeyChord {
    pub fn parse(chord_str: &str) -> Result<Vec<Self>> {
        let mut mods = Modifiers::NONE;
        let mut chord = chord_str;

        while let Some((modifier, rest)) = Self::strip_mod(chord) {
            mods |= match modifier {
                "C" => Modifiers::CTRL,
                "S" => Modifiers::SHIFT,
                "M" => Modifiers::ALT,
                _ => unreachable!(),
            };
            chord = rest;
        }

        let key = chord;
        if key.is_empty() {
            bail!("{chord_str:?} binds no key");
        }

        let keys: Box<dyn Iterator<Item = Result<_>>> = match Self::expand_range(key) {
            Some(keys) => Box::new(
                keys.with_context(|| format!("invalid key range in {chord_str:?}"))?
                    .map(|key| KeyOrPointerButton::parse(&key)),
            ),
            None => Box::new(iter::once(KeyOrPointerButton::parse(key))),
        };

        keys
            .map(|key| key.map(|key| KeyChord { key, mods }))
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("invalid key in {chord_str:?}"))
    }

    fn strip_mod(name: &str) -> Option<(&str, &str)> {
        ["C-", "S-", "M-"]
            .into_iter()
            .find_map(|prefix| name.strip_prefix(prefix).map(|key| (&prefix[..1], key)))
    }

    fn expand_range(name: &str) -> Option<Result<Box<dyn Iterator<Item = String>>>> {
        let (start, end) = name.split_once("…")?;

        Some((|| -> Result<Box<dyn Iterator<Item = _>>> {
            if start.len() > 1 {
                bail!("start of range {name:?} must be a single ascii character");
            }
            let Some(start) = start.as_bytes().first() else {
                bail!("missing start of range {name:?}");
            };
            let start = *start as char;

            if end.len() > 1 {
                bail!("end of range must be a single ascii character {name:?}");
            }
            let Some(end) = end.as_bytes().first() else {
                bail!("missing end of range {name:?}");
            };
            let end = *end as char;

            if start > end {
                bail!("start of range {name:?} is greater than its end");
            }

            Ok(Box::new((start..=end).map(|c| c.to_string())))
        })())
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum KeyOrPointerButton {
    Key(Key),
    PointerButton(PointerButton),
}

impl KeyOrPointerButton {
    pub fn name(&self) -> Cow<'static, str> {
        match self {
            Self::Key(key) => match key {
                &k if k >= Key::A && k <= Key::Z => Cow::Owned(key.name().to_lowercase()),
                Key::ArrowUp => Cow::Borrowed("↑"),
                Key::ArrowDown => Cow::Borrowed("↓"),
                Key::ArrowLeft => Cow::Borrowed("←"),
                Key::ArrowRight => Cow::Borrowed("→"),
                _ => Cow::Borrowed(key.symbol_or_name()),
            },
            Self::PointerButton(button) => Cow::Borrowed(match button {
                PointerButton::Primary => "<pointer-1>",
                PointerButton::Middle => "<pointer-2>",
                PointerButton::Secondary => "<pointer-3>",
                PointerButton::Extra1 => "<pointer-4>",
                PointerButton::Extra2 => "<pointer-5>",
            }),
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        if let Some(key) = match name {
            "↑" => Some(Key::ArrowUp),
            "↓" => Some(Key::ArrowDown),
            "←" => Some(Key::ArrowLeft),
            "→" => Some(Key::ArrowRight),
            _ => None,
        } {
            return Ok(KeyOrPointerButton::Key(key));
        }

        if let Some(digit) = name
            .strip_prefix("<pointer-")
            .and_then(|name| name.strip_suffix('>'))
        {
            return Ok(KeyOrPointerButton::PointerButton(match digit {
                "1" => PointerButton::Primary,
                "2" => PointerButton::Middle,
                "3" => PointerButton::Secondary,
                "4" => PointerButton::Extra1,
                "5" => PointerButton::Extra2,
                _ => bail!("invalid pointer button {name:?}"),
            }));
        }

        Key::from_name(name)
            .map(KeyOrPointerButton::Key)
            .ok_or_else(|| anyhow!("invalid key {name:?}"))
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ScrollAction {
    ItemUp,
    ItemDown,
    HalfUp,
    HalfDown,
    PageUp,
    PageDown,
    ToTop,
    ToBottom,
}
impl ScrollAction {
    pub fn flipped(self) -> Self {
        use ScrollAction::*;
        match self {
            ItemUp => ItemDown,
            ItemDown => ItemUp,
            HalfUp => HalfDown,
            HalfDown => HalfUp,
            PageUp => PageDown,
            PageDown => PageUp,
            ToTop => ToBottom,
            ToBottom => ToTop,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum SimpleScrollAction {
    Up,
    Down,
}

#[derive(Debug, Default, PartialEq, Eq, Copy, Clone)]
pub struct PasteModifier {
    pub trim: bool,
    pub and_enter: bool,
}
impl PasteModifier {
    pub const NONE: Self = Self {
        trim: false,
        and_enter: false,
    };
    pub const TRIM: Self = Self {
        trim: true,
        and_enter: false,
    };
    pub const AND_ENTER: Self = Self {
        trim: false,
        and_enter: true,
    };
    pub const TRIM_AND_ENTER: Self = Self {
        trim: true,
        and_enter: true,
    };
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Action {
    Key(KeyAction),
    Pointer(PointerAction),
    Passthrough,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum KeyAction {
    Paste(PasteModifier),
    QuickPaste(usize, PasteModifier),
    Scroll(ScrollAction),
    Remove,
    Pin,
    SimpleScroll(SimpleScrollAction),
    ShowSearch,
    ShowHelp,
    Close,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum PointerAction {
    Paste(PasteModifier),
}
