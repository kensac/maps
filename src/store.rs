//! An array that is either owned (a map built from an extract, or read from
//! a v1 snapshot) or borrowed from a memory-mapped v2 snapshot.

use std::ops::Deref;

pub enum Store<T: 'static> {
    Owned(Vec<T>),
    /// Borrowed from a mapping that lives for the rest of the process.
    Mapped(&'static [T]),
}

impl<T> Store<T> {
    /// Whether the data is in the mapped file rather than on the heap.
    pub fn is_mapped(&self) -> bool {
        matches!(self, Store::Mapped(_))
    }
}

impl<T> Default for Store<T> {
    fn default() -> Self {
        Store::Owned(Vec::new())
    }
}

impl<T> Deref for Store<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Store::Owned(v) => v,
            Store::Mapped(s) => s,
        }
    }
}

impl<T> From<Vec<T>> for Store<T> {
    fn from(v: Vec<T>) -> Self {
        Store::Owned(v)
    }
}

impl<'a, T> IntoIterator for &'a Store<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
