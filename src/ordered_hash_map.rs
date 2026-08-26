use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

use bincode::{Decode, Encode};

#[derive(Debug, Default, Decode, Encode)]
pub struct OrderedHashMap<K, V>
where
    K: Eq + Hash + Clone,
{
    map: HashMap<K, V>,
    keys: VecDeque<K>,
}

impl<K, V> OrderedHashMap<K, V>
where
    K: Eq + Hash + Clone,
{
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            keys: VecDeque::new(),
        }
    }

    pub fn push_front(&mut self, key: K, value: V) -> Option<V> {
        self.remove_in_keys(&key);
        self.keys.push_front(key.clone());
        self.map.insert(key, value)
    }

    pub fn push_back(&mut self, key: K, value: V) -> Option<V> {
        self.remove_in_keys(&key);
        self.keys.push_back(key.clone());
        self.map.insert(key, value)
    }

    pub fn insert(&mut self, index: usize, key: K, value: V) -> Option<V> {
        self.remove_in_keys(&key);
        self.keys.insert(index, key.clone());
        self.map.insert(key, value)
    }

    fn remove_in_keys(&mut self, key: &K) {
        if self.map.contains_key(key)
            && let Some(pos) = self.keys.iter().position(|k| k == key)
        {
            self.keys.remove(pos);
        }
    }

    pub fn pop_front(&mut self) -> Option<(K, V)> {
        self.keys
            .pop_front()
            .and_then(|k| self.map.remove(&k).map(|v| (k, v)))
    }

    pub fn pop_back(&mut self) -> Option<(K, V)> {
        self.keys
            .pop_back()
            .and_then(|k| self.map.remove(&k).map(|v| (k, v)))
    }

    pub fn front(&self) -> Option<(&K, &V)> {
        self.keys
            .front()
            .and_then(|k| self.map.get(k).map(|v| (k, v)))
    }

    pub fn back(&self) -> Option<(&K, &V)> {
        self.keys
            .back()
            .and_then(|k| self.map.get(k).map(|v| (k, v)))
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        let mut other = Self::new();
        let split_keys = self.keys.split_off(at);
        for key in split_keys {
            if let Some(value) = self.map.remove(&key) {
                other.push_back(key, value);
            }
        }
        other
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    pub fn get_by_index(&self, index: usize) -> Option<(&K, &V)> {
        self.keys.get(index).and_then(|k| self.map.get_key_value(k))
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let value = self.map.remove(key);
        if value.is_some()
            && let Some(pos) = self.keys.iter().position(|k| k == key)
        {
            self.keys.remove(pos);
        }
        value
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.keys.clear();
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            map: &self.map,
            keys: &self.keys,
            idx: 0,
            back_idx: 0,
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut {
            map: &mut self.map,
            keys: &self.keys,
            idx: 0,
            back_idx: 0,
        }
    }

    pub fn binary_search_by<'a, F>(&'a self, mut f: F) -> Result<usize, usize>
    where
        F: FnMut((&'a K, &'a V)) -> Ordering,
    {
        self.keys
            .binary_search_by(|k| f((k, self.map.get(k).unwrap())))
    }
}

// -----

pub struct Iter<'a, K, V> {
    map: &'a HashMap<K, V>,
    keys: &'a VecDeque<K>,
    idx: usize,
    back_idx: usize,
}

impl<'a, K, V> Iterator for Iter<'a, K, V>
where
    K: Eq + Hash + Clone,
{
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.keys.len() - self.back_idx {
            let key = &self.keys[self.idx];
            self.idx += 1;
            if let Some(val) = self.map.get(key) {
                return Some((key, val));
            }

            // If key was removed from map but still in keys vec, skip it
        }

        None
    }
}

pub struct IterMut<'a, K, V> {
    map: *mut HashMap<K, V>,
    keys: &'a VecDeque<K>,
    idx: usize,
    back_idx: usize,
}

impl<'a, K, V> Iterator for IterMut<'a, K, V>
where
    K: Eq + Hash + Clone,
    V: 'a,
{
    type Item = (&'a K, &'a mut V);
    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.keys.len() - self.back_idx {
            let key = &self.keys[self.idx];
            self.idx += 1;
            // SAFETY: `self.keys` contains no duplicates, so each call to get_mut()
            // returns a &mut V for a different entry
            let val = unsafe { (*self.map).get_mut(key) };
            if let Some(val) = val {
                return Some((key, val));
            }

            // If key was removed from map but still in keys vec, skip it
        }
        None
    }
}

impl<'a, K, V> DoubleEndedIterator for Iter<'a, K, V>
where
    K: Eq + Hash + Clone,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        while self.back_idx < self.keys.len() - self.idx {
            let key = &self.keys[self.keys.len() - self.back_idx - 1];
            self.back_idx += 1;
            if let Some(val) = self.map.get(key) {
                return Some((key, val));
            }

            // If key was removed from map but still in keys vec, skip it
        }

        None
    }
}

impl<'a, K, V> ExactSizeIterator for Iter<'a, K, V>
where
    K: Eq + Hash + Clone,
{
    fn len(&self) -> usize {
        self.map.len() - self.idx - self.back_idx
    }
}

// -----

impl<'a, K, V> IntoIterator for &'a OrderedHashMap<K, V>
where
    K: Eq + Hash + Clone,
{
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

// -----

impl<K, V> IntoIterator for OrderedHashMap<K, V>
where
    K: Eq + Hash + Clone,
{
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { ordered_map: self }
    }
}

pub struct IntoIter<K, V>
where
    K: Eq + Hash + Clone,
{
    ordered_map: OrderedHashMap<K, V>,
}

impl<K, V> Iterator for IntoIter<K, V>
where
    K: Eq + Hash + Clone,
{
    type Item = (K, V);
    fn next(&mut self) -> Option<Self::Item> {
        self.ordered_map.pop_front()
    }
}
