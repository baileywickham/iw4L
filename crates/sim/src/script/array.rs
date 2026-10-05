//! A script array's storage. GSC arrays are mostly lists (`a[a.size] = x`),
//! and an ordered map made every element access a tree search; the run of
//! integer keys from zero lives in a vector here, every other key in the
//! ordered map. Iteration, size and every lookup answer exactly as the one
//! ordered map did: negative integers, then 0.., then the rest.

use super::{ArrayKey, BTreeMap, Value};

#[derive(Clone, Debug, Default)]
pub(crate) struct ScriptArray {
    /// Keys `0..dense.len()`; `None` is an absent key (a removed element).
    dense: Vec<Option<Value>>,
    /// The `Some` entries of `dense`.
    present: usize,
    /// Every other key. Holds no integer in `0..dense.len()`.
    rest: BTreeMap<ArrayKey, Value>,
}

fn dense_index(key: &ArrayKey) -> Option<usize> {
    match key {
        ArrayKey::Integer(i) => usize::try_from(*i).ok(),
        ArrayKey::String(_) => None,
    }
}

impl ScriptArray {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A list: keys `0..values.len()`.
    pub(crate) fn from_values(values: Vec<Value>) -> Self {
        Self {
            present: values.len(),
            dense: values.into_iter().map(Some).collect(),
            rest: BTreeMap::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.present + self.rest.len()
    }

    pub(crate) fn get(&self, key: &ArrayKey) -> Option<&Value> {
        match dense_index(key) {
            Some(at) if at < self.dense.len() => self.dense[at].as_ref(),
            _ => self.rest.get(key),
        }
    }

    pub(crate) fn contains_key(&self, key: &ArrayKey) -> bool {
        self.get(key).is_some()
    }

    pub(crate) fn insert(&mut self, key: ArrayKey, value: Value) -> Option<Value> {
        match dense_index(&key) {
            Some(at) if at < self.dense.len() => {
                let old = self.dense[at].replace(value);
                if old.is_none() {
                    self.present += 1;
                }
                old
            }
            Some(at) if at == self.dense.len() => {
                let old = self.rest.remove(&key);
                self.dense.push(Some(value));
                self.present += 1;
                // Keys that were past the end join the run once it reaches them.
                while let Some(next) = i32::try_from(self.dense.len())
                    .ok()
                    .and_then(|next| self.rest.remove(&ArrayKey::Integer(next)))
                {
                    self.dense.push(Some(next));
                    self.present += 1;
                }
                old
            }
            _ => self.rest.insert(key, value),
        }
    }

    pub(crate) fn remove(&mut self, key: &ArrayKey) -> Option<Value> {
        match dense_index(key) {
            Some(at) if at < self.dense.len() => {
                let old = self.dense[at].take();
                if old.is_some() {
                    self.present -= 1;
                    while self.dense.last().is_some_and(Option::is_none) {
                        self.dense.pop();
                    }
                }
                old
            }
            _ => self.rest.remove(key),
        }
    }

    fn dense_end(&self) -> ArrayKey {
        ArrayKey::Integer(i32::try_from(self.dense.len()).unwrap_or(i32::MAX))
    }

    /// Entries in key order.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = (ArrayKey, &Value)> + '_ {
        let negative = self
            .rest
            .range(..ArrayKey::Integer(0))
            .map(|(key, value)| (key.clone(), value));
        let dense = self
            .dense
            .iter()
            .enumerate()
            .filter_map(|(at, value)| Some((ArrayKey::Integer(at as i32), value.as_ref()?)));
        let tail = self
            .rest
            .range(self.dense_end()..)
            .map(|(key, value)| (key.clone(), value));
        negative.chain(dense).chain(tail)
    }

    pub(crate) fn keys(&self) -> impl DoubleEndedIterator<Item = ArrayKey> + '_ {
        self.iter().map(|(key, _)| key)
    }

    pub(crate) fn values(&self) -> impl DoubleEndedIterator<Item = &Value> + '_ {
        self.iter().map(|(_, value)| value)
    }

    /// The greatest key.
    pub(crate) fn last_key(&self) -> Option<ArrayKey> {
        self.keys().next_back()
    }

    /// The greatest key below `key`.
    pub(crate) fn key_before(&self, key: &ArrayKey) -> Option<ArrayKey> {
        let negative = || {
            self.rest
                .range(..ArrayKey::Integer(0))
                .next_back()
                .map(|(key, _)| key.clone())
        };
        let dense_below = |end: usize| {
            self.dense[..end]
                .iter()
                .rposition(Option::is_some)
                .map(|at| ArrayKey::Integer(at as i32))
        };
        match dense_index(key) {
            Some(at) if at < self.dense.len() => dense_below(at).or_else(negative),
            _ if *key < ArrayKey::Integer(0) => self
                .rest
                .range(..key.clone())
                .next_back()
                .map(|(key, _)| key.clone()),
            _ => self
                .rest
                .range(self.dense_end()..key.clone())
                .next_back()
                .map(|(key, _)| key.clone())
                .or_else(|| dense_below(self.dense.len()))
                .or_else(negative),
        }
    }
}

impl FromIterator<(ArrayKey, Value)> for ScriptArray {
    fn from_iter<I: IntoIterator<Item = (ArrayKey, Value)>>(entries: I) -> Self {
        let mut array = Self::new();
        for (key, value) in entries {
            array.insert(key, value);
        }
        array
    }
}

impl IntoIterator for ScriptArray {
    type Item = (ArrayKey, Value);
    type IntoIter = std::vec::IntoIter<(ArrayKey, Value)>;
    fn into_iter(self) -> Self::IntoIter {
        let mut out = Vec::with_capacity(self.len());
        let mut rest = self.rest;
        let tail = rest.split_off(&ArrayKey::Integer(0));
        out.extend(rest);
        out.extend(
            self.dense
                .into_iter()
                .enumerate()
                .filter_map(|(at, value)| Some((ArrayKey::Integer(at as i32), value?))),
        );
        out.extend(tail);
        out.into_iter()
    }
}
