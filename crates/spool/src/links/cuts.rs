//! Where a graph of links can be parted: the links that alone hold a
//! part of it on, and the fewest links between two groups of members.

use std::collections::VecDeque;

use super::{Edge, Graph, MOST_CUT, Parting};

impl Graph {
    /// The links that alone hold a part on to the rest: removing any one
    /// of them parts the identity. Of those, the `most` whose smaller side
    /// is the largest, each with its two sides, the smaller first. The
    /// sides are found only for those, as a chain of thousands of records
    /// has as many bridges.
    pub(crate) fn bridges(&self, most: usize) -> Vec<Parting> {
        let n = self.len();
        // Depth first, numbering each member in the order it is reached.
        // The members below one in that walk are numbered next after it.
        let mut order = vec![u32::MAX; n];
        let mut low = vec![0u32; n];
        let mut below = vec![1u32; n];
        let mut root_of = vec![0u32; n];
        let mut found: Vec<(u32, u32)> = Vec::new();
        let mut counter = 0;
        let mut part_size = vec![0u32; n];
        for root in 0..n as u32 {
            if order[root as usize] != u32::MAX {
                continue;
            }
            let first = counter;
            // Without recursion: a member, the edge it was reached by, and
            // how far through its links it has gone.
            let mut stack: Vec<(u32, u32, usize)> = vec![(root, u32::MAX, 0)];
            order[root as usize] = counter;
            low[root as usize] = counter;
            root_of[root as usize] = root;
            counter += 1;
            while let Some(&mut (at, by, ref mut next)) = stack.last_mut() {
                if let Some(&(to, edge)) = self.adjacent[at as usize].get(*next) {
                    *next += 1;
                    if edge == by {
                        continue;
                    }
                    if order[to as usize] == u32::MAX {
                        order[to as usize] = counter;
                        low[to as usize] = counter;
                        root_of[to as usize] = root;
                        counter += 1;
                        stack.push((to, edge, 0));
                    } else {
                        low[at as usize] = low[at as usize].min(order[to as usize]);
                    }
                } else {
                    stack.pop();
                    if let Some(&(parent, _, _)) = stack.last() {
                        low[parent as usize] = low[parent as usize].min(low[at as usize]);
                        below[parent as usize] += below[at as usize];
                        if low[at as usize] > order[parent as usize] {
                            found.push((by, at));
                        }
                    }
                }
            }
            part_size[root as usize] = counter - first;
        }

        let smaller = |child: u32| {
            let under = below[child as usize];
            under.min(part_size[root_of[child as usize] as usize] - under)
        };
        found.sort_by(|&(one, below_one), &(other, below_other)| {
            smaller(below_other)
                .cmp(&smaller(below_one))
                .then_with(|| one.cmp(&other))
        });
        found.truncate(most);
        found
            .into_iter()
            .map(|(edge, child)| {
                let (first, end) = (
                    order[child as usize],
                    order[child as usize] + below[child as usize],
                );
                let root = root_of[child as usize];
                let (under, rest): (Vec<u32>, Vec<u32>) = (0..n as u32)
                    .filter(|&at| root_of[at as usize] == root)
                    .partition(|&at| (first..end).contains(&order[at as usize]));
                let (one, other) = match under.len() <= rest.len() {
                    true => (under, rest),
                    false => (rest, under),
                };
                Parting {
                    edges: vec![edge],
                    one,
                    other,
                }
            })
            .collect()
    }

    /// The fewest links whose removal would part every member of `one`
    /// from every member of `other`, with the two sides of the part they are
    /// in. `None` when the two share a member, when no chain joins them, or
    /// when more than [`MOST_CUT`] links would have to go.
    ///
    /// Of the cuts with that few links, the one whose smaller side is the
    /// larger: a cut that parts one record from the rest says less about
    /// where two things were joined than one that parts two groups.
    #[expect(clippy::too_many_lines)]
    pub(crate) fn cut(&self, one: &[u32], other: &[u32]) -> Option<Parting> {
        let mut sink = vec![false; self.len()];
        for &at in other {
            sink[at as usize] = true;
        }
        if one.is_empty() || other.is_empty() || one.iter().any(|&at| sink[at as usize]) {
            return None;
        }
        // The flow along each edge: 1 from `a` to `b`, -1 back, or 0.
        let mut flow = vec![0i8; self.edges.len()];
        let residual = |flow: &[i8], from: u32, edge: u32| {
            let sent = flow[edge as usize];
            if self.edges[edge as usize].a == from {
                sent < 1
            } else {
                sent > -1
            }
        };
        const START: u32 = u32::MAX;
        let mut paths = 0;
        loop {
            let mut reached: Vec<Option<(u32, u32)>> = vec![None; self.len()];
            let mut queue = VecDeque::new();
            for &at in one {
                reached[at as usize] = Some((START, START));
                queue.push_back(at);
            }
            let mut end = None;
            while let Some(at) = queue.pop_front() {
                if sink[at as usize] {
                    end = Some(at);
                    break;
                }
                for &(to, edge) in &self.adjacent[at as usize] {
                    if reached[to as usize].is_none() && residual(&flow, at, edge) {
                        reached[to as usize] = Some((at, edge));
                        queue.push_back(to);
                    }
                }
            }
            let Some(mut at) = end else { break };
            if paths == MOST_CUT {
                return None;
            }
            paths += 1;
            while let Some((before, edge)) =
                reached[at as usize].filter(|&(before, _)| before != START)
            {
                flow[edge as usize] += if self.edges[edge as usize].a == before {
                    1
                } else {
                    -1
                };
                at = before;
            }
        }
        if paths == 0 {
            return None;
        }

        // Every part a member of either group is in, so that the links
        // listed part the two groups in each.
        let mut within = vec![false; self.len()];
        let mut queue: VecDeque<u32> = one.iter().chain(other).copied().collect();
        for &at in &queue {
            within[at as usize] = true;
        }
        while let Some(at) = queue.pop_front() {
            for &(to, _) in &self.adjacent[at as usize] {
                if !within[to as usize] {
                    within[to as usize] = true;
                    queue.push_back(to);
                }
            }
        }
        let part: Vec<u32> = (0..self.len() as u32)
            .filter(|&at| within[at as usize])
            .collect();

        // Every cut of this many links is a side that holds what `one` still
        // reaches, and with each member everything it still reaches, and
        // nothing that reaches `other`. Groups of members that reach each
        // other are added whole, each after every group it reaches, and of
        // the sides that leaves, the most even is kept.
        let mut inside = vec![false; self.len()];
        let mut queue: VecDeque<u32> = one.iter().copied().collect();
        for &at in one {
            inside[at as usize] = true;
        }
        while let Some(at) = queue.pop_front() {
            for &(to, edge) in &self.adjacent[at as usize] {
                if !inside[to as usize] && residual(&flow, at, edge) {
                    inside[to as usize] = true;
                    queue.push_back(to);
                }
            }
        }
        let mut toward = vec![false; self.len()];
        let mut queue: VecDeque<u32> = other.iter().copied().collect();
        for &at in other {
            toward[at as usize] = true;
        }
        while let Some(at) = queue.pop_front() {
            for &(to, edge) in &self.adjacent[at as usize] {
                if !toward[to as usize] && residual(&flow, to, edge) {
                    toward[to as usize] = true;
                    queue.push_back(to);
                }
            }
        }
        let free: Vec<u32> = part
            .iter()
            .copied()
            .filter(|&at| !inside[at as usize] && !toward[at as usize])
            .collect();
        let groups = self.reaching_groups(&free, &|from, edge| residual(&flow, from, edge));
        let total = part.len();
        let mut size = part.iter().filter(|&&at| inside[at as usize]).count();
        let (mut best, mut best_at) = (size.min(total - size), 0);
        for (at, group) in groups.iter().enumerate() {
            size += group.len();
            if size.min(total - size) > best {
                (best, best_at) = (size.min(total - size), at + 1);
            }
        }
        for group in &groups[..best_at] {
            for &member in group {
                inside[member as usize] = true;
            }
        }
        let (with_one, with_other): (Vec<u32>, Vec<u32>) =
            part.iter().partition(|&&at| inside[at as usize]);
        let edges = (0..self.edges.len() as u32)
            .filter(|&edge| {
                let Edge { a, b, .. } = self.edges[edge as usize];
                inside[a as usize] != inside[b as usize]
            })
            .collect();
        Some(Parting {
            edges,
            one: with_one,
            other: with_other,
        })
    }

    /// The groups of `members` that reach each other along the arcs `open`
    /// lets through, each group before any group that reaches it. Arcs to
    /// members not given are not followed.
    fn reaching_groups(&self, members: &[u32], open: &dyn Fn(u32, u32) -> bool) -> Vec<Vec<u32>> {
        let n = self.len();
        let mut given = vec![false; n];
        for &at in members {
            given[at as usize] = true;
        }
        // Tarjan's algorithm without recursion, which finishes a group only
        // after every group it reaches.
        let mut index = vec![u32::MAX; n];
        let mut low = vec![0u32; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<u32> = Vec::new();
        let mut groups = Vec::new();
        let mut counter = 0;
        for &root in members {
            if index[root as usize] != u32::MAX {
                continue;
            }
            let mut walk: Vec<(u32, usize)> = vec![(root, 0)];
            index[root as usize] = counter;
            low[root as usize] = counter;
            counter += 1;
            stack.push(root);
            on_stack[root as usize] = true;
            while let Some(&mut (at, ref mut next)) = walk.last_mut() {
                if let Some(&(to, edge)) = self.adjacent[at as usize].get(*next) {
                    *next += 1;
                    if !given[to as usize] || !open(at, edge) {
                        continue;
                    }
                    if index[to as usize] == u32::MAX {
                        index[to as usize] = counter;
                        low[to as usize] = counter;
                        counter += 1;
                        stack.push(to);
                        on_stack[to as usize] = true;
                        walk.push((to, 0));
                    } else if on_stack[to as usize] {
                        low[at as usize] = low[at as usize].min(index[to as usize]);
                    }
                } else {
                    walk.pop();
                    if let Some(&(parent, _)) = walk.last() {
                        low[parent as usize] = low[parent as usize].min(low[at as usize]);
                    }
                    if low[at as usize] == index[at as usize] {
                        let mut group = Vec::new();
                        while let Some(member) = stack.pop() {
                            on_stack[member as usize] = false;
                            group.push(member);
                            if member == at {
                                break;
                            }
                        }
                        groups.push(group);
                    }
                }
            }
        }
        groups
    }

    /// The fewest links that part the records named as `one` is from those
    /// named as `other` is, given as `ones` and `others`. Only when the two
    /// groups share a record, which they do when the two have one name, is
    /// it the cut between `one` and `other` themselves: a cut between two
    /// records mostly parts one from its own name, which says nothing of
    /// where two names were joined.
    pub(crate) fn cut_between_names(
        &self,
        one: u32,
        other: u32,
        ones: &[u32],
        others: &[u32],
    ) -> Option<Parting> {
        let shared = ones.iter().any(|at| others.contains(at));
        match shared {
            true => self.cut(&[one], &[other]),
            false => self.cut(ones, others),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::tests::{clique, graph};

    #[test]
    fn two_groups_joined_by_one_link_have_it_as_their_bridge() {
        let mut links = clique(&[0, 1, 2]);
        links.extend(clique(&[3, 4, 5, 6]));
        links.push((2, 3));
        let graph = graph(7, &links);
        let bridges = graph.bridges(5);
        let [bridge] = &bridges[..] else {
            panic!("one bridge, found {}", bridges.len());
        };
        assert_eq!(graph.edges[bridge.edges[0] as usize].a, 2);
        assert_eq!(bridge.one, [0, 1, 2]);
        assert_eq!(bridge.other, [3, 4, 5, 6]);
    }

    #[test]
    fn the_cut_between_two_groups_is_the_links_between_them_not_a_lone_member() {
        // Two groups joined by two links, and a member hanging on the first
        // by one link, which is a smaller cut of the far member but parts
        // nothing but itself.
        let mut links = clique(&[0, 1, 2, 3]);
        links.extend(clique(&[4, 5, 6, 7]));
        links.extend([(2, 4), (3, 5), (8, 0), (8, 1)]);
        let graph = graph(9, &links);
        let parting = graph.cut(&[8], &[7]).unwrap();
        let mut cut: Vec<(u32, u32)> = parting
            .edges
            .iter()
            .map(|&edge| (graph.edges[edge as usize].a, graph.edges[edge as usize].b))
            .collect();
        cut.sort();
        assert_eq!(cut, [(2, 4), (3, 5)]);
        assert_eq!(parting.one, [0, 1, 2, 3, 8]);
        assert_eq!(parting.other, [4, 5, 6, 7]);
    }

    #[test]
    fn a_cut_between_groups_keeps_each_group_whole() {
        // Each lone member has fewer links than the two groups have between
        // them, so a cut between two members would part one from its own.
        let mut links = clique(&[0, 1, 2]);
        links.extend(clique(&[3, 4, 5]));
        links.extend([(0, 3), (1, 4), (2, 5), (6, 0), (7, 5)]);
        let graph = graph(8, &links);
        assert_eq!(graph.cut(&[6], &[7]).unwrap().edges.len(), 1);
        let parting = graph.cut(&[0, 1, 2], &[3, 4, 5]).unwrap();
        assert_eq!(parting.edges.len(), 3);
        assert_eq!(parting.one, [0, 1, 2, 6]);
        assert_eq!(parting.other, [3, 4, 5, 7]);
        assert!(
            graph.cut(&[0, 1], &[1, 2]).is_none(),
            "the groups share a member"
        );
    }

    #[test]
    fn of_the_cuts_of_a_chain_the_most_even_is_kept() {
        let graph = graph(6, &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5)]);
        let parting = graph.cut(&[0], &[5]).unwrap();
        assert_eq!(parting.one, [0, 1, 2]);
        assert_eq!(parting.other, [3, 4, 5]);
        assert_eq!(parting.edges, [2]);
    }

    #[test]
    fn a_cut_between_groups_parts_them_in_every_part_they_are_in() {
        let graph = graph(4, &[(0, 1), (2, 3)]);
        let parting = graph.cut(&[0, 2], &[1, 3]).unwrap();
        assert_eq!(parting.edges, [0, 1]);
        assert_eq!(parting.one, [0, 2]);
        assert_eq!(parting.other, [1, 3]);
    }

    #[test]
    fn two_names_held_by_more_links_than_are_looked_for_are_not_cut_as_two_records() {
        // Two groups of ten, every member of one linked to every member
        // of the other but its own counterpart: far more than the cap.
        let first: Vec<u32> = (0..10).collect();
        let second: Vec<u32> = (10..20).collect();
        let mut links = clique(&first);
        links.extend(clique(&second));
        for &a in &first {
            for &b in &second {
                if b != a + 10 {
                    links.push((a, b));
                }
            }
        }
        let graph = graph(20, &links);
        assert!(graph.cut_between_names(0, 10, &first, &second).is_none());
        assert!(graph.cut(&[0], &[10]).is_some(), "a cut of one record is");
        // With one name, the groups are the same, and the two records are
        // cut from each other.
        let small = super::tests::graph(3, &[(0, 1), (1, 2)]);
        let parting = small
            .cut_between_names(0, 2, &[0, 1, 2], &[0, 1, 2])
            .unwrap();
        assert_eq!(parting.edges.len(), 1);
    }

    #[test]
    fn of_the_bridges_of_a_long_chain_only_the_most_even_are_kept() {
        let n = 20_000;
        let links: Vec<(u32, u32)> = (0..n - 1).map(|at| (at, at + 1)).collect();
        let graph = graph(n, &links);
        let bridges = graph.bridges(3);
        assert_eq!(bridges.len(), 3);
        for bridge in &bridges {
            assert!(
                bridge.one.len() >= (n as usize / 2) - 1,
                "{}",
                bridge.one.len()
            );
            assert_eq!(bridge.one.len() + bridge.other.len(), n as usize);
        }
    }

    #[test]
    fn members_held_by_more_links_than_are_looked_for_have_no_cut() {
        let members: Vec<u32> = (0..(MOST_CUT as u32 + 3)).collect();
        let graph = graph(members.len() as u32, &clique(&members));
        assert!(graph.cut(&[0], &[1]).is_none());
        assert!(graph.bridges(5).is_empty());
    }
}
