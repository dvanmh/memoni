use std::{borrow::Cow, collections::HashMap, fmt, mem, sync::LazyLock};

use anyhow::Result;
use egui::{Event, Key, Modifiers, PointerButton, RawInput};
use log::{debug, trace};

use crate::{AppMode, utils::is_char_key};

pub struct KeymapEntry {
    pub keys: Vec<KeyChord>,
    pub action: Action,
    pub description: &'static str,
}

pub struct KeymapGroup {
    pub mode: Option<AppMode>,
    pub name: &'static str,
    pub entries: Vec<KeymapEntry>,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum KeyOrPointerButton {
    Key(Key),
    PointerButton(PointerButton),
}
impl KeyOrPointerButton {
    pub fn name(&self) -> Cow<'static, str> {
        match self {
            KeyOrPointerButton::Key(key) => match key {
                &k if k >= Key::A && k <= Key::Z => Cow::Owned(key.name().to_lowercase()),
                Key::ArrowUp => Cow::Borrowed("↑"),
                Key::ArrowDown => Cow::Borrowed("↓"),
                Key::ArrowLeft => Cow::Borrowed("←"),
                Key::ArrowRight => Cow::Borrowed("→"),
                _ => Cow::Borrowed(key.symbol_or_name()),
            },
            KeyOrPointerButton::PointerButton(button) => Cow::Borrowed(match button {
                PointerButton::Primary => "<pointer-1>",
                PointerButton::Middle => "<pointer-2>",
                PointerButton::Secondary => "<pointer-3>",
                PointerButton::Extra1 => "<pointer-4>",
                PointerButton::Extra2 => "<pointer-5>",
            }),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct KeyChord {
    pub key: KeyOrPointerButton,
    pub mods: Modifiers,
}
impl KeyChord {
    pub fn of_key_chord(key: Key, mods: Modifiers) -> KeyChord {
        KeyChord {
            key: KeyOrPointerButton::Key(key),
            mods,
        }
    }

    pub fn of_key(key: Key) -> KeyChord {
        Self::of_key_chord(key, Modifiers::NONE)
    }

    pub fn of_ptr_btn_chord(btn: PointerButton, mods: Modifiers) -> KeyChord {
        KeyChord {
            key: KeyOrPointerButton::PointerButton(btn),
            mods,
        }
    }

    pub fn of_ptr_btn(btn: PointerButton) -> KeyChord {
        Self::of_ptr_btn_chord(btn, Modifiers::NONE)
    }
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

#[derive(Debug, Copy, Clone)]
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

#[derive(Debug, Copy, Clone)]
pub enum SimpleScrollAction {
    Up,
    Down,
}

#[derive(Debug, Default, Copy, Clone)]
pub struct PasteModifier {
    pub trim: bool,
    pub and_enter: bool,
}

#[derive(Debug, Copy, Clone)]
pub enum Action {
    Key(KeyAction),
    Pointer(PointerAction),
    Passthrough,
}

#[derive(Debug, Copy, Clone)]
pub enum KeyAction {
    Paste(PasteModifier),
    QuickPaste(usize),
    Scroll(ScrollAction),
    Remove,
    Pin,
    SimpleScroll(SimpleScrollAction),
    ShowSearch,
    ShowHelp,
    Close,
}

#[derive(Debug, Copy, Clone)]
pub enum PointerAction {
    Paste(PasteModifier),
}

#[rustfmt::skip]
pub static ACTION_KEYMAPS: LazyLock<Vec<KeymapGroup>> = LazyLock::new(|| {
    use Action::Key as AK;
    use Action::Pointer as AP;
    use KeyAction::*;
    use Key::*;
    use PointerButton::*;
    use KeyChord as KC;
    use Modifiers as M;

    macro_rules! e {
        ($keys:expr, $action:expr, $desc:expr) => {
            KeymapEntry { keys: $keys, action: $action, description: $desc }
        };
    }

    vec![
        KeymapGroup {
            mode: None,
            name: "General",
            entries: vec![
                e!(vec![KC::of_ptr_btn(Primary)],         AP(PointerAction::Paste(PasteModifier::default())),
                    "Paste item"),
                e!(vec![KC::of_ptr_btn_chord(Primary, M::CTRL)],
                                                          AP(PointerAction::Paste(PasteModifier { and_enter: true, trim: false })),
                                                                                                "Paste item and press Enter"),
                e!(vec![KC::of_ptr_btn_chord(Primary, M::SHIFT)],
                                                          AP(PointerAction::Paste(PasteModifier { trim: true, and_enter: false })),
                                                                                                "Paste trimmed item"),
                e!(vec![KC::of_ptr_btn_chord(Primary, M::SHIFT | M::CTRL)],
                                                          AP(PointerAction::Paste(PasteModifier { trim: true, and_enter: true })),
                                                                                                "Paste trimmed item and press Enter"),
            ],
        },
        KeymapGroup {
            mode: Some(AppMode::Normal),
            name: "Normal Mode",
            entries: vec![
                e!(vec![KC::of_key(ArrowUp)],             AK(Scroll(ScrollAction::ItemUp)),     "Move to previous item"),
                e!(vec![KC::of_key(ArrowDown)],           AK(Scroll(ScrollAction::ItemDown)),   "Move to next item"),
                e!(vec![KC::of_key(K)],                   AK(Scroll(ScrollAction::ItemUp)),     "Move to previous item"),
                e!(vec![KC::of_key(J)],                   AK(Scroll(ScrollAction::ItemDown)),   "Move to next item"),
                e!(vec![KC::of_key_chord(P, M::CTRL)],    AK(Scroll(ScrollAction::ItemUp)),     "Move to previous item"),
                e!(vec![KC::of_key_chord(N, M::CTRL)],    AK(Scroll(ScrollAction::ItemDown)),   "Move to next item"),
                e!(vec![KC::of_key_chord(Tab, M::SHIFT)], AK(Scroll(ScrollAction::ItemUp)),     "Move to previous item"),
                e!(vec![KC::of_key(Tab)],                 AK(Scroll(ScrollAction::ItemDown)),   "Move to next item"),

                e!(vec![KC::of_key_chord(U, M::CTRL)],    AK(Scroll(ScrollAction::HalfUp)),     "Scroll half page up"),
                e!(vec![KC::of_key_chord(D, M::CTRL)],    AK(Scroll(ScrollAction::HalfDown)),   "Scroll half page down"),

                e!(vec![KC::of_key_chord(B, M::CTRL)],    AK(Scroll(ScrollAction::PageUp)),     "Scroll page up"),
                e!(vec![KC::of_key_chord(F, M::CTRL)],    AK(Scroll(ScrollAction::PageDown)),   "Scroll page down"),

                e!(vec![KC::of_key(G), KC::of_key(G)],    AK(Scroll(ScrollAction::ToTop)),      "Go to first item"),
                e!(vec![KC::of_key_chord(G, M::SHIFT)],   AK(Scroll(ScrollAction::ToBottom)),   "Go to last item"),

                e!(vec![KC::of_key(Enter)],               AK(KeyAction::Paste(PasteModifier::default())),
                                                                                                "Paste item"),
                e!(vec![KC::of_key(Space)],               AK(KeyAction::Paste(PasteModifier::default())),
                                                                                                "Paste item"),
                e!(vec![KC::of_ptr_btn(Primary)],         AP(PointerAction::Paste(PasteModifier::default())),
                                                                                                "Paste item"),

                e!(vec![KC::of_key_chord(Enter, M::CTRL)],
                                                          AK(KeyAction::Paste(PasteModifier { and_enter: true, trim: false })),
                                                                                                "Paste item and press Enter"),
                e!(vec![KC::of_key_chord(Space, M::CTRL)],
                                                          AK(KeyAction::Paste(PasteModifier { and_enter: true, trim: false })),
                                                                                                "Paste item and press Enter"),
                e!(vec![KC::of_ptr_btn_chord(Primary, M::CTRL)],
                                                          AP(PointerAction::Paste(PasteModifier { and_enter: true, trim: false })),
                                                                                                "Paste item and press Enter"),

                e!(vec![KC::of_key_chord(Enter, M::SHIFT)],
                                                          AK(KeyAction::Paste(PasteModifier { trim: true, and_enter: false })),
                                                                                                "Paste trimmed item"),
                e!(vec![KC::of_key_chord(Space, M::SHIFT)],
                                                          AK(KeyAction::Paste(PasteModifier { trim: true, and_enter: false })),
                                                                                                "Paste trimmed item"),
                e!(vec![KC::of_ptr_btn_chord(Primary, M::SHIFT)],
                                                          AP(PointerAction::Paste(PasteModifier { trim: true, and_enter: false })),
                                                                                                "Paste trimmed item"),

                e!(vec![KC::of_key_chord(Enter, M::SHIFT | M::CTRL)],
                                                          AK(KeyAction::Paste(PasteModifier { trim: true, and_enter: true })),
                                                                                                "Paste trimmed item and press Enter"),
                e!(vec![KC::of_key_chord(Space, M::SHIFT | M::CTRL)],
                                                          AK(KeyAction::Paste(PasteModifier { trim: true, and_enter: true })),
                                                                                                "Paste trimmed item and press Enter"),
                e!(vec![KC::of_ptr_btn_chord(Primary, M::SHIFT | M::CTRL)],
                                                          AP(PointerAction::Paste(PasteModifier { trim: true, and_enter: true })),
                                                                                                "Paste trimmed item and press Enter"),

                e!(vec![KC::of_key(Num1)],                AK(QuickPaste(0)),                    "Quick paste item 1"),
                e!(vec![KC::of_key(Num2)],                AK(QuickPaste(1)),                    "Quick paste item 2"),
                e!(vec![KC::of_key(Num3)],                AK(QuickPaste(2)),                    "Quick paste item 3"),
                e!(vec![KC::of_key(Num4)],                AK(QuickPaste(3)),                    "Quick paste item 4"),
                e!(vec![KC::of_key(Num5)],                AK(QuickPaste(4)),                    "Quick paste item 5"),
                e!(vec![KC::of_key(Num6)],                AK(QuickPaste(5)),                    "Quick paste item 6"),
                e!(vec![KC::of_key(Num7)],                AK(QuickPaste(6)),                    "Quick paste item 7"),
                e!(vec![KC::of_key(Num8)],                AK(QuickPaste(7)),                    "Quick paste item 8"),
                e!(vec![KC::of_key(Num9)],                AK(QuickPaste(8)),                    "Quick paste item 9"),
                e!(vec![KC::of_key(Num0)],                AK(QuickPaste(9)),                    "Quick paste item 10"),

                e!(vec![KC::of_key(D), KC::of_key(D)],    AK(Remove),                           "Remove item"),
                e!(vec![KC::of_key(Delete)],              AK(Remove),                           "Remove item"),

                e!(vec![KC::of_key(P)],                   AK(Pin),                              "Toggle pin"),

                e!(vec![KC::of_key(Escape)],              AK(Close),                            "Close window"),
                e!(vec![KC::of_key(Q)],                   AK(Close),                            "Close window"),

                e!(vec![KC::of_key(Slash)],               AK(ShowSearch),                       "Open search bar"),
                e!(vec![KC::of_key_chord(Slash, M::SHIFT)],
                                                          AK(ShowHelp),                         "Open help modal"),
            ],
        },
        KeymapGroup {
            mode: Some(AppMode::Search),
            name: "Search Mode",
            entries: vec![
                e!(vec![KC::of_key(ArrowUp)],             AK(Scroll(ScrollAction::ItemUp)),     "Move to previous item"),
                e!(vec![KC::of_key(ArrowDown)],           AK(Scroll(ScrollAction::ItemDown)),   "Move to next item"),
                e!(vec![KC::of_key_chord(P, M::CTRL)],    AK(Scroll(ScrollAction::ItemUp)),     "Move to previous item"),
                e!(vec![KC::of_key_chord(N, M::CTRL)],    AK(Scroll(ScrollAction::ItemDown)),   "Move to next item"),

                e!(vec![KC::of_key(Enter)],               AK(KeyAction::Paste(PasteModifier::default())),
                                                                                                "Paste item"),
                e!(vec![KC::of_key_chord(Enter, M::CTRL)],
                                                          AK(KeyAction::Paste(PasteModifier { and_enter: true, trim: false })),
                                                                                                "Paste item and press Enter"),
                e!(vec![KC::of_key_chord(Enter, M::SHIFT)],
                                                          AK(KeyAction::Paste(PasteModifier { trim: true, and_enter: false })),
                                                                                                "Paste trimmed item"),
                e!(vec![KC::of_key_chord(Enter, M::SHIFT | M::CTRL)],
                                                          AK(KeyAction::Paste(PasteModifier { trim: true, and_enter: true })),
                                                                                                "Paste trimmed item and press Enter"),

                e!(vec![KC::of_key_chord(Num1, M::ALT)],  AK(QuickPaste(0)),                    "Quick paste item 1"),
                e!(vec![KC::of_key_chord(Num2, M::ALT)],  AK(QuickPaste(1)),                    "Quick paste item 2"),
                e!(vec![KC::of_key_chord(Num3, M::ALT)],  AK(QuickPaste(2)),                    "Quick paste item 3"),
                e!(vec![KC::of_key_chord(Num4, M::ALT)],  AK(QuickPaste(3)),                    "Quick paste item 4"),
                e!(vec![KC::of_key_chord(Num5, M::ALT)],  AK(QuickPaste(4)),                    "Quick paste item 5"),
                e!(vec![KC::of_key_chord(Num6, M::ALT)],  AK(QuickPaste(5)),                    "Quick paste item 6"),
                e!(vec![KC::of_key_chord(Num7, M::ALT)],  AK(QuickPaste(6)),                    "Quick paste item 7"),
                e!(vec![KC::of_key_chord(Num8, M::ALT)],  AK(QuickPaste(7)),                    "Quick paste item 8"),
                e!(vec![KC::of_key_chord(Num9, M::ALT)],  AK(QuickPaste(8)),                    "Quick paste item 9"),
                e!(vec![KC::of_key_chord(Num0, M::ALT)],  AK(QuickPaste(9)),                    "Quick paste item 10"),

                e!(vec![KC::of_key(ArrowLeft)],           Action::Passthrough,                  "Move caret one character left"),
                e!(vec![KC::of_key(ArrowRight)],          Action::Passthrough,                  "Move caret one character right"),
                e!(vec![KC::of_key_chord(ArrowLeft, M::CTRL)],
                                                          Action::Passthrough,                  "Move caret one word left"),
                e!(vec![KC::of_key_chord(ArrowRight, M::CTRL)],
                                                          Action::Passthrough,                  "Move caret one word right"),
                e!(vec![KC::of_key_chord(ArrowLeft, M::SHIFT)],
                                                          Action::Passthrough,                  "Extend selection one character left"),
                e!(vec![KC::of_key_chord(ArrowRight, M::SHIFT)],
                                                          Action::Passthrough,                  "Extend selection one character right"),
                e!(vec![KC::of_key_chord(ArrowLeft, M::CTRL | M::SHIFT)],
                                                          Action::Passthrough,                  "Extend selection one word left"),
                e!(vec![KC::of_key_chord(ArrowRight, M::CTRL | M::SHIFT)],
                                                          Action::Passthrough,                  "Extend selection one word right"),

                e!(vec![KC::of_key(Home)],                Action::Passthrough,                  "Move caret to start of line"),
                e!(vec![KC::of_key(End)],                 Action::Passthrough,                  "Move caret to end of line"),
                e!(vec![KC::of_key_chord(Home, M::SHIFT)],
                                                          Action::Passthrough,                  "Select to start of line"),
                e!(vec![KC::of_key_chord(End, M::SHIFT)],
                                                          Action::Passthrough,                  "Select to end of line"),

                e!(vec![KC::of_key(Backspace)],           Action::Passthrough,                  "Delete character left"),
                e!(vec![KC::of_key(Delete)],              Action::Passthrough,                  "Delete character right"),
                e!(vec![KC::of_key_chord(Backspace, M::CTRL)],
                                                          Action::Passthrough,                  "Delete word left"),
                e!(vec![KC::of_key_chord(Delete, M::CTRL)],
                                                          Action::Passthrough,                  "Delete word right"),

                e!(vec![KC::of_key(Escape)],              AK(Close),                            "Exit search"),
            ],
        },
        KeymapGroup {
            mode: Some(AppMode::Help),
            name: "Help Mode",
            entries: vec![
                e!(vec![KC::of_key(ArrowUp)],             AK(SimpleScroll(SimpleScrollAction::Up)),
                                                                                                "Scroll up"),
                e!(vec![KC::of_key(ArrowDown)],           AK(SimpleScroll(SimpleScrollAction::Down)),
                                                                                                "Scroll down"),
                e!(vec![KC::of_key(K)],                   AK(SimpleScroll(SimpleScrollAction::Up)),
                                                                                                "Scroll up"),
                e!(vec![KC::of_key(J)],                   AK(SimpleScroll(SimpleScrollAction::Down)),
                                                                                                "Scroll down"),
                e!(vec![KC::of_key_chord(P, M::CTRL)],    AK(SimpleScroll(SimpleScrollAction::Up)),
                                                                                                "Scroll up"),
                e!(vec![KC::of_key_chord(N, M::CTRL)],    AK(SimpleScroll(SimpleScrollAction::Down)),
                                                                                                "Scroll down"),

                e!(vec![KC::of_key(Escape)],              AK(Close),                            "Close help"),
                e!(vec![KC::of_key(Q)],                   AK(Close),                            "Close help"),
            ],
        },
    ]
});

pub struct KeymapAction {
    action_keymap_tries: HashMap<AppMode, Trie<&'static KeyChord, Action>>,
    pub pending_keys: Vec<KeyChord>,
}
impl KeymapAction {
    pub fn new() -> Result<Self> {
        let mut action_keymap_tries = HashMap::new();

        for group in ACTION_KEYMAPS.iter() {
            if let Some(mode) = group.mode {
                let trie = action_keymap_tries
                    .entry(mode)
                    .or_insert_with(Trie::default);
                for entry in &group.entries {
                    trie.insert(&entry.keys, entry.action);
                }
            }
        }

        for group in ACTION_KEYMAPS.iter() {
            if group.mode.is_none() {
                for trie in action_keymap_tries.values_mut() {
                    for entry in &group.entries {
                        trie.insert(&entry.keys, entry.action);
                    }
                }
            }
        }

        Ok(KeymapAction {
            action_keymap_tries,
            pending_keys: vec![],
        })
    }

    pub fn process_input(
        &mut self,
        egui_input: &mut RawInput,
        mode: AppMode,
    ) -> (Vec<KeyAction>, Vec<PointerAction>) {
        let mut key_actions = vec![];
        let mut pointer_actions = vec![];

        let trie = match self.action_keymap_tries.get(&mode) {
            Some(trie) => trie,
            None => return (key_actions, pointer_actions),
        };

        for event in mem::take(&mut egui_input.events) {
            let key_chord = match event {
                event @ Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } => {
                    if pressed
                        && !(mode == AppMode::Search
                            && is_char_key(key)
                            && (modifiers.is_none() || modifiers.shift_only()))
                    {
                        Some((KeyChord::of_key_chord(key, modifiers), event))
                    } else {
                        None
                    }
                }
                event @ Event::PointerButton {
                    button,
                    pressed,
                    modifiers,
                    ..
                } => {
                    // pointer action activated on button release
                    if !pressed {
                        egui_input.events.push(event.clone());
                        Some((KeyChord::of_ptr_btn_chord(button, modifiers), event))
                    } else {
                        egui_input.events.push(event);
                        None
                    }
                }
                event @ Event::Text(_) => {
                    if mode == AppMode::Search {
                        trace!("text event provided to egui: {event:?}");
                        egui_input.events.push(event);
                    }
                    None
                }
                event => {
                    egui_input.events.push(event);
                    None
                }
            };

            if let Some((key_chord, event)) = key_chord {
                if key_chord.key == KeyOrPointerButton::Key(Key::Escape)
                    && !self.pending_keys.is_empty()
                {
                    debug!("received Escape, clearing pending keys");
                    self.pending_keys.clear();
                    continue;
                }

                self.pending_keys.push(key_chord);
                if let Some(keymap_node) = trie.get_node(&self.pending_keys) {
                    if let Some(action) = keymap_node.value {
                        debug!(
                            "converting keymap {:?} to action {action:?}",
                            self.pending_keys
                        );
                        match action {
                            Action::Key(key_action) => key_actions.push(key_action),
                            Action::Pointer(pointer_action) => pointer_actions.push(pointer_action),
                            Action::Passthrough => egui_input.events.push(event),
                        }
                        self.pending_keys.clear();
                    } else {
                        debug!("continuing building keymap: {:?}", self.pending_keys);
                    }
                } else {
                    debug!("received invalid keymap: {:?}", self.pending_keys);
                    self.pending_keys.clear();
                }
            }
        }

        (key_actions, pointer_actions)
    }
}

// Extremely simple trie implementation

struct Trie<K, V> {
    value: Option<V>,
    next: Vec<(K, Trie<K, V>)>,
}

impl<K, V> Default for Trie<K, V> {
    fn default() -> Self {
        Trie {
            value: None,
            next: vec![],
        }
    }
}

impl<K, V> Trie<K, V>
where
    K: PartialEq + Clone,
{
    fn insert<I>(&mut self, keys: I, value: V)
    where
        I: IntoIterator<Item = K>,
    {
        let mut node = self;
        for key in keys {
            if let Some(pos) = node.next.iter().position(|(k, _)| *k == key) {
                node = &mut node.next[pos].1;
            } else {
                node.next.push((key.clone(), Trie::default()));
                let len = node.next.len();
                node = &mut node.next[len - 1].1;
            }
        }

        node.value = Some(value);
    }

    fn get_node<I>(&self, keys: I) -> Option<&Trie<K, V>>
    where
        I: IntoIterator<Item = K>,
    {
        let mut node = self;
        for key in keys {
            match node.next.iter().find(|(k, _)| *k == key) {
                Some((_, child)) => node = child,
                None => return None,
            }
        }

        Some(node)
    }
}
