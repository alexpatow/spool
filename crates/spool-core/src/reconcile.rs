//! Turns judgements about new observations into a draft changeset.
//!
//! Strategies supply evidence. This module decides what that evidence changes,
//! and it is the only place where pairwise judgements become groups.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::changeset::{Changeset, Op};
use crate::generate::IdGenerator;
use crate::ids::{Actor, EntityType, IdentityId, ObservationId};
use crate::state::{IdentityState, IdentityStatus, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Match,
    NoMatch,
    Unsure,
}

impl Verdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Verdict::Match => "match",
            Verdict::NoMatch => "no_match",
            Verdict::Unsure => "unsure",
        }
    }
}

impl std::str::FromStr for Verdict {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "match" => Ok(Verdict::Match),
            "no_match" => Ok(Verdict::NoMatch),
            "unsure" => Ok(Verdict::Unsure),
            other => Err(format!(
                "unknown verdict {other:?}: expected `match`, `no_match` or `unsure`"
            )),
        }
    }
}

/// What a match run found out about its new observations, already joined
/// against current membership by the store. It is small: one row per new
/// observation, per pair of new observations that were judged, and per
/// identity a new observation was judged against.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceSummary {
    pub observations: Vec<NewObservation>,
    pub pairs: Vec<NewPair>,
    pub links: Vec<IdentityLink>,
}

/// An observation the run was told is new.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewObservation {
    pub observation: ObservationId,
    pub entity_type: EntityType,
}

/// A judgement between two new observations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPair {
    pub a: ObservationId,
    pub b: ObservationId,
    pub verdict: Verdict,
}

/// The judgements between one new observation and the members of one identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityLink {
    pub observation: ObservationId,
    pub identity: IdentityId,
    pub matches: u64,
    pub no_matches: u64,
    /// The best score among the matches, on the strategy's own scale.
    pub best_score: Option<f64>,
}

/// Reconciliation policy. The default policy has no settings yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Policy {}

/// A changeset that has not been committed, and what it deliberately left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    /// May have no operations, in which case there is nothing to commit.
    pub changeset: Changeset,
    pub review: Vec<ReviewItem>,
}

/// Something the reconciler would not decide. The observations named are left
/// without an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewItem {
    /// The observations match members of more than one identity. Those
    /// identities may be the same real thing, which only a reviewer can say.
    PossibleMerge {
        observations: Vec<ObservationId>,
        identities: Vec<IdentityId>,
    },
    /// The evidence contradicts itself: the observations match each other or
    /// `identity`, and a `no_match` judgement says some of them do not belong
    /// together, or do not belong with a member of `identity`.
    Conflict {
        observations: Vec<ObservationId>,
        identity: Option<IdentityId>,
    },
}

/// The default policy. New observations that match each other form a group,
/// and a group is decided as a whole:
///
/// 1. A group that matches members of exactly one identity joins it.
/// 2. A group that matches no identity becomes a new identity. A group of one
///    is the common case: an observation that matched nothing.
/// 3. A group that matches members of two or more identities is left
///    unassigned, with a [`ReviewItem::PossibleMerge`].
/// 4. A group with a `no_match` judgement inside it, or between it and the
///    identity it matches, is left unassigned, with a [`ReviewItem::Conflict`].
/// 5. Groups that would join the same identity, with a `no_match` judgement
///    between them, are left unassigned together, with one
///    [`ReviewItem::Conflict`]. Joining would put observations that were
///    judged different into one identity.
///
/// Existing identities are never merged or split, and an observation that
/// already has an identity is never moved. `unsure` judgements change nothing.
///
/// The output depends only on the inputs: groups are visited in order of their
/// smallest observation ID, so the same evidence and the same ID generator
/// give the same draft.
pub fn reconcile<S: IdentityState + ?Sized>(
    evidence: &EvidenceSummary,
    state: &S,
    _policy: &Policy,
    ids: &mut dyn IdGenerator,
) -> Result<Draft> {
    let base = state.head()?;

    // Only observations that still have no identity are new.
    let candidates: Vec<ObservationId> = evidence
        .observations
        .iter()
        .map(|new| new.observation)
        .collect();
    let assigned = state.assignments(&candidates)?;
    let new: BTreeMap<ObservationId, &EntityType> = evidence
        .observations
        .iter()
        .zip(assigned)
        .filter(|(_, identity)| identity.is_none())
        .map(|(new, _)| (new.observation, &new.entity_type))
        .collect();

    // Only active identities can be joined.
    let mut linked: Vec<IdentityId> = evidence.links.iter().map(|link| link.identity).collect();
    linked.sort_unstable();
    linked.dedup();
    let active: BTreeSet<IdentityId> = state
        .identities(&linked)?
        .into_iter()
        .flatten()
        .filter(|record| record.status == IdentityStatus::Active)
        .map(|record| record.id)
        .collect();

    let mut groups = Groups::new(new.keys().copied());
    for pair in &evidence.pairs {
        if pair.verdict == Verdict::Match && new.contains_key(&pair.a) && new.contains_key(&pair.b)
        {
            groups.join(pair.a, pair.b);
        }
    }

    let mut matched: BTreeMap<ObservationId, BTreeSet<IdentityId>> = BTreeMap::new();
    let mut refused: BTreeMap<ObservationId, BTreeSet<IdentityId>> = BTreeMap::new();
    for link in &evidence.links {
        if !new.contains_key(&link.observation) || !active.contains(&link.identity) {
            continue;
        }
        let root = groups.root(link.observation);
        if link.matches > 0 {
            matched.entry(root).or_default().insert(link.identity);
        }
        if link.no_matches > 0 {
            refused.entry(root).or_default().insert(link.identity);
        }
    }
    let split_inside: BTreeSet<ObservationId> = evidence
        .pairs
        .iter()
        .filter(|pair| pair.verdict == Verdict::NoMatch)
        .filter(|pair| new.contains_key(&pair.a) && new.contains_key(&pair.b))
        .filter(|pair| groups.root(pair.a) == groups.root(pair.b))
        .map(|pair| groups.root(pair.a))
        .collect();

    // What each group would do if it were the only one.
    let members = groups.members();
    let none = BTreeSet::new();
    let mut decisions: BTreeMap<ObservationId, Decision> = BTreeMap::new();
    for (root, group) in &members {
        let matched = matched.get(root).unwrap_or(&none);
        let refused = refused.get(root).unwrap_or(&none);
        let mut matched_ids = matched.iter().copied();
        let target = matched_ids.next();
        let decision = if matched_ids.next().is_some() {
            Decision::Review(ReviewItem::PossibleMerge {
                observations: group.clone(),
                identities: matched.iter().copied().collect(),
            })
        } else if split_inside.contains(root) || target.is_some_and(|id| refused.contains(&id)) {
            Decision::Review(ReviewItem::Conflict {
                observations: group.clone(),
                identity: target,
            })
        } else {
            match target {
                Some(identity) => Decision::Join(identity),
                None => Decision::Create,
            }
        };
        decisions.insert(*root, decision);
    }

    // Groups that would join the same identity become members of one thing,
    // so a `no_match` between two of them is a contradiction, just as it
    // would be inside a group. Groups it connects are left for review
    // together. Groups that become new identities stay apart, so a
    // `no_match` between those contradicts nothing.
    let mut clashes = Groups::new(members.keys().copied());
    let mut clashing = BTreeSet::new();
    for pair in &evidence.pairs {
        if pair.verdict != Verdict::NoMatch
            || !new.contains_key(&pair.a)
            || !new.contains_key(&pair.b)
        {
            continue;
        }
        let (a, b) = (groups.root(pair.a), groups.root(pair.b));
        if a == b {
            continue;
        }
        if let (Decision::Join(left), Decision::Join(right)) = (&decisions[&a], &decisions[&b])
            && left == right
        {
            clashes.join(a, b);
            clashing.extend([a, b]);
        }
    }
    let mut clash_members = clashes.members();

    let mut ops = Vec::new();
    let mut review = Vec::new();
    for (root, group) in members {
        let Some(decision) = decisions.remove(&root) else {
            continue;
        };
        let target = match decision {
            Decision::Review(item) => {
                review.push(item);
                continue;
            }
            Decision::Join(identity) if clashing.contains(&root) => {
                // One item for the groups a chain of `no_match` connects,
                // raised when the first of them is visited.
                if let Some(roots) = clash_members.remove(&root) {
                    let mut observations: Vec<ObservationId> = roots
                        .iter()
                        .flat_map(|root| groups.members_of(*root))
                        .collect();
                    observations.sort_unstable();
                    review.push(ReviewItem::Conflict {
                        observations,
                        identity: Some(identity),
                    });
                }
                continue;
            }
            Decision::Join(identity) => identity,
            Decision::Create => {
                let identity = ids.identity_id();
                ops.push(Op::CreateIdentity {
                    identity,
                    entity_type: new[&group[0]].clone(),
                });
                identity
            }
        };
        ops.extend(group.into_iter().map(|observation| Op::Assign {
            observation,
            from: None,
            to: Some(target),
        }));
    }

    Ok(Draft {
        changeset: Changeset {
            id: ids.changeset_id(),
            base,
            actor: Actor::Engine,
            reason: Some("reconcile new observations".to_owned()),
            run: None,
            ops,
        },
        review,
    })
}

/// What a group of new observations would do, judged alone.
enum Decision {
    Review(ReviewItem),
    Join(IdentityId),
    Create,
}

/// Connected components over the new observations.
struct Groups {
    parent: BTreeMap<ObservationId, ObservationId>,
}

impl Groups {
    fn new(observations: impl Iterator<Item = ObservationId>) -> Self {
        Self {
            parent: observations.map(|id| (id, id)).collect(),
        }
    }

    fn root(&self, mut id: ObservationId) -> ObservationId {
        while self.parent[&id] != id {
            id = self.parent[&id];
        }
        id
    }

    /// The smaller root wins, so a group's root is its smallest member.
    fn join(&mut self, a: ObservationId, b: ObservationId) {
        let (a, b) = (self.root(a), self.root(b));
        let (keep, drop) = if a <= b { (a, b) } else { (b, a) };
        self.parent.insert(drop, keep);
    }

    /// Each group's root with its members, both in ascending order.
    fn members(&self) -> BTreeMap<ObservationId, Vec<ObservationId>> {
        let mut members: BTreeMap<ObservationId, Vec<ObservationId>> = BTreeMap::new();
        for id in self.parent.keys() {
            members.entry(self.root(*id)).or_default().push(*id);
        }
        members
    }

    /// The members of the group with this root.
    fn members_of(&self, root: ObservationId) -> impl Iterator<Item = ObservationId> + '_ {
        self.parent
            .keys()
            .copied()
            .filter(move |id| self.root(*id) == root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::SeededIds;
    use crate::memory::MemoryState;
    use crate::state::IdentityStore;
    use crate::validate::validate;

    fn widget() -> EntityType {
        EntityType::new("widget").unwrap()
    }

    fn obs(n: u128) -> ObservationId {
        ObservationId::from_u128(n)
    }

    fn new(ids: &[u128]) -> Vec<NewObservation> {
        ids.iter()
            .map(|n| NewObservation {
                observation: obs(*n),
                entity_type: widget(),
            })
            .collect()
    }

    fn pair(a: u128, b: u128, verdict: Verdict) -> NewPair {
        NewPair {
            a: obs(a),
            b: obs(b),
            verdict,
        }
    }

    fn link(
        observation: u128,
        identity: IdentityId,
        matches: u64,
        no_matches: u64,
    ) -> IdentityLink {
        IdentityLink {
            observation: obs(observation),
            identity,
            matches,
            no_matches,
            best_score: None,
        }
    }

    /// A state holding observations 1 to 20, where each listed group already
    /// is an identity.
    fn state_with(groups: &[&[u128]]) -> (MemoryState, Vec<IdentityId>) {
        let mut state = MemoryState::new();
        for n in 1..=20 {
            state.observe(obs(n), widget());
        }
        let mut ids = SeededIds::new(1000);
        let mut identities = Vec::new();
        for group in groups {
            let identity = ids.identity_id();
            let mut ops = vec![Op::CreateIdentity {
                identity,
                entity_type: widget(),
            }];
            ops.extend(group.iter().map(|n| Op::Assign {
                observation: obs(*n),
                from: None,
                to: Some(identity),
            }));
            let changeset = Changeset {
                id: ids.changeset_id(),
                base: state.head().unwrap(),
                actor: Actor::Engine,
                reason: None,
                run: None,
                ops,
            };
            state.apply(validate(&state, changeset).unwrap()).unwrap();
            identities.push(identity);
        }
        (state, identities)
    }

    fn run(evidence: &EvidenceSummary, state: &MemoryState) -> Draft {
        reconcile(evidence, state, &Policy::default(), &mut SeededIds::new(1)).unwrap()
    }

    fn assigned_to(draft: &Draft, observation: u128) -> Option<IdentityId> {
        draft.changeset.ops.iter().find_map(|op| match op {
            Op::Assign {
                observation: o, to, ..
            } if *o == obs(observation) => *to,
            _ => None,
        })
    }

    fn created(draft: &Draft) -> Vec<IdentityId> {
        draft
            .changeset
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::CreateIdentity { identity, .. } => Some(*identity),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_observation_that_matches_one_identity_joins_it() {
        let (state, identities) = state_with(&[&[1, 2]]);
        let evidence = EvidenceSummary {
            observations: new(&[3]),
            pairs: vec![],
            links: vec![link(3, identities[0], 2, 0)],
        };
        let draft = run(&evidence, &state);
        assert_eq!(
            draft.changeset.ops,
            vec![Op::Assign {
                observation: obs(3),
                from: None,
                to: Some(identities[0])
            }]
        );
        assert!(draft.review.is_empty());
        assert_eq!(draft.changeset.base, state.head().unwrap());
    }

    #[test]
    fn observations_that_match_only_each_other_form_one_new_identity() {
        let (state, _) = state_with(&[]);
        let evidence = EvidenceSummary {
            observations: new(&[1, 2, 3, 4]),
            pairs: vec![pair(1, 2, Verdict::Match), pair(2, 3, Verdict::Match)],
            links: vec![],
        };
        let draft = run(&evidence, &state);
        let made = created(&draft);
        assert_eq!(made.len(), 2, "one for the group and one for observation 4");
        assert_eq!(assigned_to(&draft, 1), assigned_to(&draft, 3));
        assert_eq!(assigned_to(&draft, 1), Some(made[0]));
        assert_eq!(assigned_to(&draft, 4), Some(made[1]));
        assert!(draft.review.is_empty());
        assert!(validate(&state, draft.changeset).is_ok());
    }

    #[test]
    fn a_group_joins_the_identity_one_of_its_members_matched() {
        let (state, identities) = state_with(&[&[1]]);
        let evidence = EvidenceSummary {
            observations: new(&[2, 3]),
            pairs: vec![pair(2, 3, Verdict::Match)],
            links: vec![link(3, identities[0], 1, 0)],
        };
        let draft = run(&evidence, &state);
        assert!(created(&draft).is_empty());
        assert_eq!(assigned_to(&draft, 2), Some(identities[0]));
        assert_eq!(assigned_to(&draft, 3), Some(identities[0]));
    }

    #[test]
    fn an_observation_that_matches_two_identities_is_left_for_review() {
        let (state, identities) = state_with(&[&[1], &[2]]);
        let evidence = EvidenceSummary {
            observations: new(&[3, 4]),
            pairs: vec![],
            links: vec![link(3, identities[0], 1, 0), link(3, identities[1], 1, 0)],
        };
        let draft = run(&evidence, &state);
        assert_eq!(assigned_to(&draft, 3), None);
        assert_eq!(
            draft.review,
            vec![ReviewItem::PossibleMerge {
                observations: vec![obs(3)],
                identities: identities.clone()
            }]
        );
        assert!(assigned_to(&draft, 4).is_some(), "the rest still proceeds");
    }

    #[test]
    fn a_group_with_a_no_match_inside_is_left_for_review() {
        let (state, _) = state_with(&[]);
        let evidence = EvidenceSummary {
            observations: new(&[1, 2, 3]),
            pairs: vec![
                pair(1, 2, Verdict::Match),
                pair(2, 3, Verdict::Match),
                pair(1, 3, Verdict::NoMatch),
            ],
            links: vec![],
        };
        let draft = run(&evidence, &state);
        assert!(draft.changeset.ops.is_empty());
        assert_eq!(
            draft.review,
            vec![ReviewItem::Conflict {
                observations: vec![obs(1), obs(2), obs(3)],
                identity: None
            }]
        );
    }

    #[test]
    fn a_no_match_against_the_matched_identity_blocks_the_assignment() {
        let (state, identities) = state_with(&[&[1, 2]]);
        let evidence = EvidenceSummary {
            observations: new(&[3]),
            pairs: vec![],
            links: vec![link(3, identities[0], 1, 1)],
        };
        let draft = run(&evidence, &state);
        assert!(draft.changeset.ops.is_empty());
        assert_eq!(
            draft.review,
            vec![ReviewItem::Conflict {
                observations: vec![obs(3)],
                identity: Some(identities[0])
            }]
        );
    }

    #[test]
    fn groups_kept_apart_by_a_no_match_do_not_join_the_same_identity() {
        let (state, identities) = state_with(&[&[1]]);
        let target = identities[0];
        // 2, 3 and 4 each match the identity. 2 and 3 are judged different.
        let evidence = EvidenceSummary {
            observations: new(&[2, 3, 4]),
            pairs: vec![pair(2, 3, Verdict::NoMatch)],
            links: vec![
                link(2, target, 1, 0),
                link(3, target, 1, 0),
                link(4, target, 1, 0),
            ],
        };
        let draft = run(&evidence, &state);
        assert_eq!(
            draft.changeset.ops,
            vec![Op::Assign {
                observation: obs(4),
                from: None,
                to: Some(target)
            }]
        );
        assert_eq!(
            draft.review,
            vec![ReviewItem::Conflict {
                observations: vec![obs(2), obs(3)],
                identity: Some(target),
            }]
        );
    }

    #[test]
    fn a_chain_of_no_matches_between_groups_is_one_review_item() {
        let (state, identities) = state_with(&[&[1], &[5]]);
        let (target, other) = (identities[0], identities[1]);
        // 2 and 3 match each other. 2-4 and 4-6 are judged different, and
        // all of them match the first identity. 7 matches the other one, so
        // its no_match with 2 contradicts nothing.
        let evidence = EvidenceSummary {
            observations: new(&[2, 3, 4, 6, 7]),
            pairs: vec![
                pair(2, 3, Verdict::Match),
                pair(2, 4, Verdict::NoMatch),
                pair(4, 6, Verdict::NoMatch),
                pair(2, 7, Verdict::NoMatch),
            ],
            links: vec![
                link(2, target, 1, 0),
                link(4, target, 1, 0),
                link(6, target, 1, 0),
                link(7, other, 1, 0),
            ],
        };
        let draft = run(&evidence, &state);
        assert_eq!(
            draft.changeset.ops,
            vec![Op::Assign {
                observation: obs(7),
                from: None,
                to: Some(other)
            }]
        );
        assert_eq!(
            draft.review,
            vec![ReviewItem::Conflict {
                observations: vec![obs(2), obs(3), obs(4), obs(6)],
                identity: Some(target),
            }]
        );
    }

    #[test]
    fn a_no_match_between_groups_that_become_new_identities_changes_nothing() {
        let (state, _) = state_with(&[]);
        let evidence = EvidenceSummary {
            observations: new(&[1, 2, 3]),
            pairs: vec![pair(1, 2, Verdict::Match), pair(2, 3, Verdict::NoMatch)],
            links: vec![],
        };
        let draft = run(&evidence, &state);
        assert_eq!(created(&draft).len(), 2);
        assert_eq!(assigned_to(&draft, 1), assigned_to(&draft, 2));
        assert_ne!(assigned_to(&draft, 1), assigned_to(&draft, 3));
        assert!(draft.review.is_empty());
    }

    #[test]
    fn a_no_match_alone_makes_a_separate_identity() {
        let (state, identities) = state_with(&[&[1]]);
        let evidence = EvidenceSummary {
            observations: new(&[2, 3]),
            pairs: vec![pair(2, 3, Verdict::NoMatch)],
            links: vec![link(2, identities[0], 0, 1)],
        };
        let draft = run(&evidence, &state);
        assert_eq!(created(&draft).len(), 2);
        assert_ne!(assigned_to(&draft, 2), assigned_to(&draft, 3));
        assert_ne!(assigned_to(&draft, 2), Some(identities[0]));
        assert!(draft.review.is_empty());
    }

    #[test]
    fn unsure_judgements_change_nothing() {
        let (state, _) = state_with(&[]);
        let evidence = EvidenceSummary {
            observations: new(&[1, 2]),
            pairs: vec![pair(1, 2, Verdict::Unsure)],
            links: vec![],
        };
        let draft = run(&evidence, &state);
        assert_eq!(created(&draft).len(), 2);
    }

    #[test]
    fn observations_that_already_have_an_identity_are_not_touched() {
        let (state, identities) = state_with(&[&[1], &[2]]);
        let evidence = EvidenceSummary {
            observations: new(&[1, 3]),
            pairs: vec![pair(1, 3, Verdict::Match)],
            links: vec![link(1, identities[1], 5, 0)],
        };
        let draft = run(&evidence, &state);
        assert_eq!(assigned_to(&draft, 1), None);
        assert_eq!(created(&draft).len(), 1, "observation 3 stands alone");
        assert!(draft.review.is_empty());
    }

    #[test]
    fn the_same_evidence_and_seed_give_the_same_draft() {
        let (state, identities) = state_with(&[&[1]]);
        let mut evidence = EvidenceSummary {
            observations: new(&[5, 2, 4, 3]),
            pairs: vec![pair(5, 4, Verdict::Match)],
            links: vec![link(2, identities[0], 1, 0)],
        };
        let first = run(&evidence, &state);
        evidence.observations.reverse();
        let second = run(&evidence, &state);
        assert_eq!(first, second);
        assert_eq!(
            first.changeset.content_hash(),
            second.changeset.content_hash()
        );
    }

    #[test]
    fn drafts_pass_validation() {
        let (state, identities) = state_with(&[&[1], &[2]]);
        let evidence = EvidenceSummary {
            observations: new(&[3, 4, 5, 6]),
            pairs: vec![pair(4, 5, Verdict::Match)],
            links: vec![link(3, identities[0], 1, 0), link(6, identities[1], 1, 0)],
        };
        let draft = run(&evidence, &state);
        assert!(validate(&state, draft.changeset).is_ok());
    }
}
