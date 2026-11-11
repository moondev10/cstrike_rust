//! `ObjectDictionary` (docs/re/hltv-world.md §5.6, demoplayer.md §12.2): objects sorted by an
//! f32 key (stable: a new entry goes after equal keys), with the closest-key search and the
//! `GetFirst`/`GetNext` cursor. The 32-entry lookup cache only short-cuts exact repeats and is
//! left out (results identical).

#[derive(Debug, Clone)]
pub struct Dict<T: Copy + PartialEq> {
    entries: Vec<(T, f32)>,
    cursor: usize,
}

impl<T: Copy + PartialEq> Default for Dict<T> {
    fn default() -> Self {
        Self { entries: Vec::new(), cursor: 0 }
    }
}

impl<T: Copy + PartialEq> Dict<T> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `Add(obj, key)`: after the last entry with `entry.key <= key`.
    pub fn add(&mut self, obj: T, key: f32) {
        let n = self.entries.len();
        if n == 0 || key >= self.entries[n - 1].1 {
            self.entries.push((obj, key));
            return;
        }
        let mut i = self.closest_index(key).max(0) as usize;
        while i < n && self.entries[i].1 <= key {
            i += 1;
        }
        self.entries.insert(i, (obj, key));
    }

    /// `Remove(obj)`.
    pub fn remove(&mut self, obj: T) -> bool {
        match self.entries.iter().position(|&(o, _)| o == obj) {
            Some(i) => {
                self.entries.remove(i);
                true
            }
            None => false,
        }
    }

    /// `ChangeKey(obj, key)`: removed and re-added with the new key.
    pub fn change_key(&mut self, obj: T, key: f32) {
        if self.remove(obj) {
            self.add(obj, key);
        }
    }

    /// `Clear`.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
    }

    /// `FindClosestAsIndex(key)`: −1 when empty; ties go to the lower entry; the first of an
    /// equal-key run.
    pub fn closest_index(&self, key: f32) -> i32 {
        let e = &self.entries;
        if e.is_empty() {
            return -1;
        }
        if e[0].1 >= key {
            return 0;
        }
        let last = e.len() - 1;
        let mut i;
        if key >= e[last].1 {
            i = last;
        } else {
            let (mut lo, mut hi) = (0usize, last);
            i = last >> 1;
            loop {
                if e[i].1 == key {
                    break;
                }
                if key > e[i].1 {
                    if e[i + 1].1 >= key {
                        if key - e[i].1 > e[i + 1].1 - key {
                            i += 1;
                        }
                        break;
                    }
                    lo = i;
                } else {
                    hi = i;
                }
                i = (lo + hi) >> 1;
                if e[i].1 == key {
                    break;
                }
            }
        }
        while i > 0 && e[i - 1].1 == key {
            i -= 1;
        }
        i as i32
    }

    /// `FindClosestKey(key)`: cursor on the closest entry, then `GetNext`.
    pub fn find_closest(&mut self, key: f32) -> Option<T> {
        let i = self.closest_index(key);
        self.cursor = if i < 0 { usize::MAX } else { i as usize };
        self.get_next()
    }

    /// `FindExactKey(key)`.
    pub fn find_exact(&mut self, key: f32) -> Option<T> {
        let i = self.closest_index(key);
        self.cursor = if i < 0 { usize::MAX } else { i as usize };
        if i >= 0 && self.entries[i as usize].1 == key { self.get_next() } else { None }
    }

    /// `GetFirst`.
    pub fn first(&mut self) -> Option<T> {
        self.cursor = 0;
        self.get_next()
    }

    /// `GetNext`.
    pub fn get_next(&mut self) -> Option<T> {
        let v = self.entries.get(self.cursor).map(|&(o, _)| o);
        if v.is_some() {
            self.cursor += 1;
        }
        v
    }

    /// `GetLast` (cursor unchanged).
    pub fn last(&self) -> Option<T> {
        self.entries.last().map(|&(o, _)| o)
    }

    /// Entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (T, f32)> + '_ {
        self.entries.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_keys_stay_in_insertion_order_and_lookups_find_the_first() {
        let mut d = Dict::default();
        for (o, k) in [(1, 1.0), (2, 2.0), (3, 2.0), (4, 3.0), (5, 2.0)] {
            d.add(o, k);
        }
        assert_eq!(d.iter().map(|e| e.0).collect::<Vec<_>>(), vec![1, 2, 3, 5, 4]);
        assert_eq!(d.find_exact(2.0), Some(2));
        assert_eq!(d.get_next(), Some(3));
        assert_eq!(d.find_exact(2.5), None);
        assert_eq!(d.find_closest(2.6), Some(4));
        assert_eq!(d.find_closest(9.0), Some(4));
        assert_eq!(d.find_closest(-1.0), Some(1));
        // The step back to the first of an equal run compares with the searched key, so coming
        // from above it stops on the last entry of the 2.0 run.
        assert_eq!(d.find_closest(2.4), Some(5));
    }
}
