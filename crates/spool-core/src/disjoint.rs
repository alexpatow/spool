//! Things that are joined in pairs, as the sets the pairs make.
//!
//! Records come together two at a time: a judgement, a constraint, a
//! decision. What they make is groups, and this is the one place that turns
//! the one into the other.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

/// Disjoint sets of items. Each item starts as a set of its own, and
/// [`join`](Self::join) makes one set of two.
///
/// A set is known by its root: of its members, the one that was put in
/// first. So what the sets are, and what each is called, does not depend on
/// the order the pairs were joined in.
#[derive(Clone, Debug)]
pub struct DisjointSets<T> {
    index: HashMap<T, usize>,
    items: Vec<T>,
    /// Each item's parent, by position. A root is its own. A parent is
    /// always earlier than its child, and looking a root up shortens the way
    /// to it, which is why this can change behind `&self`.
    parent: Vec<Cell<usize>>,
}

impl<T> Default for DisjointSets<T> {
    fn default() -> Self {
        Self {
            index: HashMap::new(),
            items: Vec::new(),
            parent: Vec::new(),
        }
    }
}

impl<T: Copy + Eq + Hash + Ord> DisjointSets<T> {
    /// Each of `items` as a set of its own.
    pub fn new(items: impl IntoIterator<Item = T>) -> Self {
        let mut sets = Self::default();
        for item in items {
            sets.insert(item);
        }
        sets
    }

    /// Adds `item` as a set of its own. One that is there already stays
    /// where it is.
    pub fn insert(&mut self, item: T) {
        if !self.index.contains_key(&item) {
            self.index.insert(item, self.items.len());
            self.parent.push(Cell::new(self.items.len()));
            self.items.push(item);
        }
    }

    fn root_at(&self, mut at: usize) -> usize {
        loop {
            let parent = self.parent[at].get();
            if parent == at {
                return at;
            }
            // Halve the way: point at the grandparent, and go on from it.
            let above = self.parent[parent].get();
            self.parent[at].set(above);
            at = above;
        }
    }

    /// The root of the set `item` is in, or `None` for an item that was
    /// never put in.
    pub fn find(&self, item: T) -> Option<T> {
        let at = *self.index.get(&item)?;
        Some(self.items[self.root_at(at)])
    }

    /// The root of the set `item` is in.
    ///
    /// # Panics
    ///
    /// If `item` was never put in.
    pub fn root(&self, item: T) -> T {
        self.find(item).expect("the item is in the sets")
    }

    /// Makes one set of the sets `a` and `b` are in. Nothing happens if
    /// either was never put in.
    pub fn join(&mut self, a: T, b: T) {
        let (Some(a), Some(b)) = (self.index.get(&a), self.index.get(&b)) else {
            return;
        };
        let (a, b) = (self.root_at(*a), self.root_at(*b));
        // The earlier root stands for both.
        self.parent[a.max(b)].set(a.min(b));
    }

    /// Every set by its root, each with its members in the order they were
    /// put in. A set of one is here too.
    pub fn sets(&self) -> BTreeMap<T, Vec<T>> {
        let mut by_root: BTreeMap<usize, Vec<T>> = BTreeMap::new();
        for (at, item) in self.items.iter().enumerate() {
            by_root.entry(self.root_at(at)).or_default().push(*item);
        }
        by_root
            .into_iter()
            .map(|(root, members)| (self.items[root], members))
            .collect()
    }

    /// The members of the set with this root, in the order they were put
    /// in.
    pub fn members_of(&self, root: T) -> impl Iterator<Item = T> + '_ {
        let root = self.index.get(&root).copied();
        self.items
            .iter()
            .enumerate()
            .filter(move |(at, _)| Some(self.root_at(*at)) == root)
            .map(|(_, item)| *item)
    }
}

#[cfg(test)]
mod tests {
    use super::DisjointSets;

    #[test]
    fn items_joined_in_pairs_are_one_set_known_by_its_first() {
        let mut sets = DisjointSets::new([10, 20, 30, 40, 50]);
        sets.join(50, 40);
        sets.join(30, 40);
        assert_eq!(sets.root(50), 30);
        assert_eq!(sets.root(40), 30);
        assert_eq!(sets.root(10), 10);
        assert_eq!(
            sets.sets().into_iter().collect::<Vec<_>>(),
            vec![(10, vec![10]), (20, vec![20]), (30, vec![30, 40, 50])]
        );
        assert_eq!(sets.members_of(30).collect::<Vec<_>>(), vec![30, 40, 50]);
    }

    #[test]
    fn the_sets_do_not_depend_on_the_order_of_the_pairs() {
        let pairs = [(1, 2), (3, 4), (2, 3), (6, 5)];
        let mut forward = DisjointSets::new(1..=6);
        let mut backward = DisjointSets::new(1..=6);
        for (a, b) in pairs {
            forward.join(a, b);
        }
        for (a, b) in pairs.iter().rev() {
            backward.join(*b, *a);
        }
        assert_eq!(forward.sets(), backward.sets());
    }

    #[test]
    fn an_item_that_was_never_put_in_is_in_no_set() {
        let mut sets = DisjointSets::new([1, 2]);
        assert_eq!(sets.find(3), None);
        sets.join(1, 3);
        assert_eq!(sets.find(1), Some(1));
        assert_eq!(sets.sets().len(), 2);
        sets.insert(3);
        sets.insert(1);
        sets.join(3, 2);
        assert_eq!(sets.root(3), 2);
    }
}
