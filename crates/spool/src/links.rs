//! The links that hold an identity together, as a graph of its members.
//!
//! A link is what joins two members directly: a `must_link` constraint in
//! force, one being a later version of the other, or a `match` judged by a
//! run that finished. An identity is held together by its links, and two
//! members that no link joins are together only through a chain of others.
//!
//! The graph is read once, with each member as a number, so that an identity
//! of thousands of members and a million links is cheap to walk: the
//! shortest chain between two members, the links that alone hold a part of
//! it on (bridges), and the fewest links whose removal would part two
//! members (a minimum cut, found as a maximum flow of unit capacities).

use std::collections::{HashMap, HashSet, VecDeque};

use duckdb::Connection;
use spool_core::{IdentityId, RecordId, RunId};

use crate::error::Result;
use crate::explain::LinkKind;
use crate::store::parse;

/// The most links a cut is looked for with. Two members that more links
/// than this hold together are not where a wrong merge entered.
pub(crate) const MOST_CUT: usize = 20;

/// One link between two members, by their numbers. `a` is the member with
/// the smaller record ID.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edge {
    pub a: u32,
    pub b: u32,
    pub kind: LinkKind,
    /// With `match`: the latest run that judged so.
    pub run: Option<RunId>,
    /// The `match` rules that run recorded as holding, as a place in
    /// [`Graph::rule_sets`]. The first set is empty.
    pub rules: u32,
}

pub(crate) struct Graph {
    /// The members, ordered by record ID. A member's number is its place here.
    pub members: Vec<RecordId>,
    pub edges: Vec<Edge>,
    /// For each member, the members it is linked to and by which edge, the
    /// strongest link first.
    adjacent: Vec<Vec<(u32, u32)>>,
    /// The names of the `match` rules the links were made by.
    pub rule_names: Vec<String>,
    /// The sets of rules links were made by, each as places in
    /// `rule_names`. The first is empty: a link that is no `match`, or
    /// whose strategy keeps no rule names.
    pub rule_sets: Vec<Vec<u32>>,
}

/// Two parts of a group of members, and the links between them.
pub(crate) struct Parting {
    /// The edges whose removal parts them.
    pub edges: Vec<u32>,
    /// The members on the side of the first member named, or of the
    /// smaller side of a bridge.
    pub one: Vec<u32>,
    /// The members on the other side.
    pub other: Vec<u32>,
}

impl Graph {
    /// The links between the members an identity has now. Between two
    /// records, a constraint is preferred to a version and a version to a
    /// judgement, which is the order in which a reconcile run listens to
    /// them, and only the strongest is kept.
    pub(crate) fn read(conn: &Connection, identity: IdentityId) -> Result<Self> {
        let mut members = Vec::new();
        let mut statement = conn.prepare(
            "SELECT record_id::VARCHAR FROM spool.membership
             WHERE to_seq IS NULL AND identity_id = ?::UUID
             ORDER BY record_id",
        )?;
        let mut rows = statement.query([identity.to_string()])?;
        while let Some(row) = rows.next()? {
            members.push(parse::<RecordId>(&row.get::<_, String>(0)?)?);
        }

        // The members are numbered in SQL, so that a million links are read
        // as numbers rather than as IDs.
        let mut statement = conn.prepare(
            "WITH member AS (
               SELECT record_id, (row_number() OVER (ORDER BY record_id) - 1)::INTEGER AS at
               FROM spool.membership
               WHERE to_seq IS NULL AND identity_id = $1::UUID),
             link AS (
               SELECT c.record_a AS one, c.record_b AS other, 0 AS kind, NULL::BIGINT AS run,
                      NULL::VARCHAR AS rules
               FROM spool.constraint_link c
               WHERE c.to_seq IS NULL AND c.kind = 'must_link'
               UNION ALL
               SELECT o.supersedes, o.record_id, 1, NULL, NULL
               FROM spool.record o
               WHERE o.supersedes IS NOT NULL
               UNION ALL
               SELECT j.left_id, j.right_id, 2, max(j.run_id),
                      arg_max(coalesce(json_extract(j.detail, '$.match')::VARCHAR, '[]'), j.run_id)
               FROM spool.judgement j
               JOIN spool.run r ON r.run_id = j.run_id AND r.status = 'succeeded'
               WHERE j.verdict = 'match'
                 AND j.left_id IN (SELECT record_id FROM member)
                 AND j.right_id IN (SELECT record_id FROM member)
               GROUP BY j.left_id, j.right_id)
             SELECT one.at, other.at, link.kind, link.run, link.rules
             FROM link
             JOIN member one ON one.record_id = link.one
             JOIN member other ON other.record_id = link.other
             WHERE one.at <> other.at
             ORDER BY link.kind, one.at, other.at",
        )?;
        let mut rows = statement.query([identity.to_string()])?;
        let mut edges = Vec::new();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        let mut rule_names: Vec<String> = Vec::new();
        let mut rule_sets: Vec<Vec<u32>> = vec![Vec::new()];
        // The few texts the rule sets are written as, each read once.
        let mut sets_of: HashMap<String, u32> = HashMap::new();
        while let Some(row) = rows.next()? {
            let (one, other) = (row.get::<_, i32>(0)? as u32, row.get::<_, i32>(1)? as u32);
            let (a, b) = (one.min(other), one.max(other));
            if !seen.insert((a, b)) {
                continue;
            }
            let kind = match row.get::<_, i32>(2)? {
                0 => LinkKind::MustLink,
                1 => LinkKind::Version,
                _ => LinkKind::Match,
            };
            let run = row.get::<_, Option<i64>>(3)?.map(|run| RunId(run as u64));
            let rules = match row.get::<_, Option<String>>(4)? {
                None => 0,
                Some(text) => match sets_of.get(&text) {
                    Some(&set) => set,
                    None => {
                        let names: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
                        let mut set: Vec<u32> = names
                            .into_iter()
                            .map(
                                |name| match rule_names.iter().position(|known| *known == name) {
                                    Some(at) => at as u32,
                                    None => {
                                        rule_names.push(name);
                                        rule_names.len() as u32 - 1
                                    }
                                },
                            )
                            .collect();
                        set.sort();
                        set.dedup();
                        let at = match rule_sets.iter().position(|known| *known == set) {
                            Some(at) => at as u32,
                            None => {
                                rule_sets.push(set);
                                rule_sets.len() as u32 - 1
                            }
                        };
                        sets_of.insert(text, at);
                        at
                    }
                },
            };
            edges.push(Edge {
                a,
                b,
                kind,
                run,
                rules,
            });
        }

        let mut adjacent = vec![Vec::new(); members.len()];
        for (at, edge) in edges.iter().enumerate() {
            adjacent[edge.a as usize].push((edge.b, at as u32));
            adjacent[edge.b as usize].push((edge.a, at as u32));
        }
        for linked in &mut adjacent {
            linked.sort_by_key(|&(to, edge)| (edges[edge as usize].kind, to));
        }
        Ok(Self {
            members,
            edges,
            adjacent,
            rule_names,
            rule_sets,
        })
    }

    /// Whether a chain joins two members without the links only `rule`
    /// made.
    pub(crate) fn together_without(&self, rule: u32, one: u32, other: u32) -> bool {
        self.pieces_without(rule)
            .iter()
            .any(|piece| piece.contains(&one) && piece.contains(&other))
    }

    /// The groups the members would be in without the links that only
    /// `rule` made, the largest first.
    pub(crate) fn pieces_without(&self, rule: u32) -> Vec<Vec<u32>> {
        let mut parent: Vec<u32> = (0..self.len() as u32).collect();
        fn root(parent: &mut [u32], mut at: u32) -> u32 {
            while parent[at as usize] != at {
                let up = parent[parent[at as usize] as usize];
                parent[at as usize] = up;
                at = up;
            }
            at
        }
        for edge in &self.edges {
            if self.rule_sets[edge.rules as usize] == [rule] {
                continue;
            }
            let (one, other) = (root(&mut parent, edge.a), root(&mut parent, edge.b));
            if one != other {
                parent[one.max(other) as usize] = one.min(other);
            }
        }
        let mut pieces: HashMap<u32, Vec<u32>> = HashMap::new();
        for at in 0..self.len() as u32 {
            let top = root(&mut parent, at);
            pieces.entry(top).or_default().push(at);
        }
        let mut pieces: Vec<Vec<u32>> = pieces.into_values().collect();
        pieces.sort_by(|one, other| other.len().cmp(&one.len()).then_with(|| one.cmp(other)));
        pieces
    }

    pub(crate) fn number(&self, record: RecordId) -> Option<u32> {
        self.members.binary_search(&record).ok().map(|at| at as u32)
    }

    pub(crate) fn len(&self) -> usize {
        self.members.len()
    }

    /// The members linked to `from`, and the number of links from it to
    /// each, counted in steps.
    fn distances(&self, from: u32) -> Vec<Option<u32>> {
        let mut distance = vec![None; self.len()];
        distance[from as usize] = Some(0);
        let mut queue = VecDeque::from([from]);
        while let Some(at) = queue.pop_front() {
            let next = distance[at as usize].map(|steps| steps + 1);
            for &(to, _) in &self.adjacent[at as usize] {
                if distance[to as usize].is_none() {
                    distance[to as usize] = next;
                    queue.push_back(to);
                }
            }
        }
        distance
    }

    /// The edges of the shortest chain from `from` to `to`, in order. Empty
    /// when no chain joins them.
    pub(crate) fn chain(&self, from: u32, to: u32) -> Vec<u32> {
        let mut reached: Vec<Option<(u32, u32)>> = vec![None; self.len()];
        let mut queue = VecDeque::from([from]);
        while let Some(at) = queue.pop_front() {
            if at == to {
                break;
            }
            for &(next, edge) in &self.adjacent[at as usize] {
                if next != from && reached[next as usize].is_none() {
                    reached[next as usize] = Some((at, edge));
                    queue.push_back(next);
                }
            }
        }
        let mut chain = Vec::new();
        let mut at = to;
        while let Some((before, edge)) = reached[at as usize] {
            chain.push(edge);
            at = before;
        }
        chain.reverse();
        chain
    }

    /// The groups of members that links hold together, the largest first.
    pub(crate) fn parts(&self) -> Vec<Vec<u32>> {
        let mut part = vec![usize::MAX; self.len()];
        let mut parts: Vec<Vec<u32>> = Vec::new();
        for start in 0..self.len() {
            if part[start] != usize::MAX {
                continue;
            }
            let mut members = vec![start as u32];
            part[start] = parts.len();
            let mut at = 0;
            while let Some(&member) = members.get(at) {
                for &(to, _) in &self.adjacent[member as usize] {
                    if part[to as usize] == usize::MAX {
                        part[to as usize] = parts.len();
                        members.push(to);
                    }
                }
                at += 1;
            }
            parts.push(members);
        }
        parts.sort_by_key(|members| std::cmp::Reverse(members.len()));
        parts
    }

    /// Two members of a part far apart, and the number of links between
    /// them: from its first member, the one farthest away, and from that
    /// one, the one farthest from it. No two members of the part are fewer
    /// steps apart than half of it, and in a chain it is the whole length.
    pub(crate) fn far_apart(&self, part: &[u32]) -> Option<(u32, u32, u32)> {
        let farthest = |from: u32| {
            self.distances(from)
                .into_iter()
                .enumerate()
                .filter_map(|(at, steps)| Some((steps?, std::cmp::Reverse(at))))
                .max()
                .map(|(steps, std::cmp::Reverse(at))| (at as u32, steps))
        };
        let (one, _) = farthest(*part.first()?)?;
        let (other, steps) = farthest(one)?;
        (steps > 0).then_some((one, other, steps))
    }

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

    /// Pairs of members a link joins directly.
    pub(crate) fn linked_pairs(&self) -> u64 {
        self.edges.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph of `n` members and the given matches.
    fn graph(n: u32, links: &[(u32, u32)]) -> Graph {
        let members = (0..n)
            .map(|at| format!("00000000-0000-7000-8000-{at:012}").parse().unwrap())
            .collect();
        let edges: Vec<Edge> = links
            .iter()
            .map(|&(a, b)| Edge {
                a: a.min(b),
                b: a.max(b),
                kind: LinkKind::Match,
                run: None,
                rules: 0,
            })
            .collect();
        let mut adjacent = vec![Vec::new(); n as usize];
        for (at, edge) in edges.iter().enumerate() {
            adjacent[edge.a as usize].push((edge.b, at as u32));
            adjacent[edge.b as usize].push((edge.a, at as u32));
        }
        Graph {
            members,
            edges,
            adjacent,
            rule_names: Vec::new(),
            rule_sets: vec![Vec::new()],
        }
    }

    /// Every pair of the given members.
    fn clique(members: &[u32]) -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        for (at, &a) in members.iter().enumerate() {
            for &b in &members[at + 1..] {
                pairs.push((a, b));
            }
        }
        pairs
    }

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
    fn a_link_has_the_rules_of_the_latest_run_that_judged_it_even_when_it_kept_none() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE SCHEMA spool;
             CREATE TABLE spool.membership (record_id UUID, identity_id UUID, to_seq BIGINT);
             CREATE TABLE spool.constraint_link (record_a UUID, record_b UUID, kind VARCHAR, to_seq BIGINT);
             CREATE TABLE spool.record (record_id UUID, supersedes UUID);
             CREATE TABLE spool.run (run_id BIGINT, status VARCHAR);
             CREATE TABLE spool.judgement (run_id BIGINT, left_id UUID, right_id UUID, verdict VARCHAR, detail JSON);
             INSERT INTO spool.membership VALUES
               ('00000000-0000-7000-8000-000000000001', '00000000-0000-7000-8000-0000000000aa', NULL),
               ('00000000-0000-7000-8000-000000000002', '00000000-0000-7000-8000-0000000000aa', NULL);
             INSERT INTO spool.run VALUES (1, 'succeeded'), (2, 'succeeded');
             INSERT INTO spool.judgement VALUES
               (1, '00000000-0000-7000-8000-000000000001', '00000000-0000-7000-8000-000000000002',
                'match', '{\"match\": [\"same_name\"]}'),
               (2, '00000000-0000-7000-8000-000000000001', '00000000-0000-7000-8000-000000000002',
                'match', NULL);",
        )
        .unwrap();
        let graph = Graph::read(
            &conn,
            "00000000-0000-7000-8000-0000000000aa".parse().unwrap(),
        )
        .unwrap();
        let [edge] = &graph.edges[..] else {
            panic!("one link");
        };
        assert_eq!(edge.run, Some(RunId(2)));
        assert!(graph.rule_sets[edge.rules as usize].is_empty());
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

    #[test]
    fn a_chain_is_the_shortest_way_and_its_ends_are_far_apart() {
        let graph = graph(5, &[(0, 1), (1, 2), (2, 3), (3, 4), (0, 2)]);
        let chain = graph.chain(0, 4);
        assert_eq!(chain.len(), 3);
        let parts = graph.parts();
        assert_eq!(parts.len(), 1);
        let (one, other, steps) = graph.far_apart(&parts[0]).unwrap();
        assert_eq!(steps, 3);
        assert_eq!([one.min(other), one.max(other)], [0, 4]);
        assert_eq!(graph.cut(&[0], &[4]).unwrap().edges.len(), 1);
    }

    #[test]
    fn members_no_link_joins_are_parts_of_their_own() {
        let graph = graph(4, &[(0, 1)]);
        let parts = graph.parts();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].len(), 2);
        assert!(graph.chain(0, 3).is_empty());
        assert!(graph.cut(&[0], &[3]).is_none());
    }
}
