use std::{collections::HashMap, mem};

use anyhow::Result;
use egui::{Event, Key, Modifiers, RawInput};
use log::{debug, trace};

use crate::keymap_spec::{
    ACTION_KEYMAPS, Action, KeyAction, KeyChord, KeyOrPointerButton, PointerAction,
};
use crate::{AppMode, utils::is_char_key};

pub struct KeymapAction {
    action_keymap_tries: HashMap<AppMode, Trie<&'static KeyChord, Action>>,
    pub pending_keys: Vec<KeyChord>,
}
impl KeymapAction {
    pub fn new() -> Result<Self> {
        let mut action_keymap_tries = HashMap::new();

        for group in ACTION_KEYMAPS.iter() {
            let trie = action_keymap_tries
                .entry(group.mode)
                .or_insert_with(Trie::default);
            for binding in &group.bindings {
                trie.insert(&binding.keys, binding.action);
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
                            && (modifiers.is_none() || modifiers == Modifiers::SHIFT))
                    {
                        Some((
                            KeyChord {
                                key: KeyOrPointerButton::Key(key),
                                mods: modifiers,
                            },
                            event,
                        ))
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
                        Some((
                            KeyChord {
                                key: KeyOrPointerButton::PointerButton(button),
                                mods: modifiers,
                            },
                            event,
                        ))
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

        // egui expects command to mirror ctrl on non-mac platforms
        for event in egui_input.events.iter_mut() {
            let modifiers = match event {
                Event::Key { modifiers, .. } => modifiers,
                Event::PointerButton { modifiers, .. } => modifiers,
                Event::MouseWheel { modifiers, .. } => modifiers,
                Event::ModifiersChanged(modifiers) => modifiers,
                _ => continue,
            };
            modifiers.command = modifiers.ctrl;
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
