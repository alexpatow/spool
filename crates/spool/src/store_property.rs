//! Random changesets, valid and invalid, must have the same outcome in the
//! in-memory state and in the DuckDB store.

use proptest::prelude::*;
use spool_core::{
    Actor, Changeset, ConstraintKind, EntityType, IdGenerator, IdentityId, IdentityState,
    IdentityStatus, IdentityStore, MemoryState, ObservationId, Op, SeededIds, StepClock, inverse,
    validate,
};

use crate::store::{DuckStore, insert_observations};

const OBSERVATIONS: u128 = 8;

/// An operation before it is bound to real IDs. Most choices are made so the
/// operation is valid, and some are left to chance so that every kind of
/// rejection turns up as well.
#[derive(Clone, Debug)]
enum OpSpec {
    Create {
        reuse: Option<usize>,
        gadget: bool,
    },
    Assign {
        observation: u128,
        from: From,
        to: To,
    },
    /// Two identities, of one entity type and active unless `any` is set.
    Merge {
        from: usize,
        into: usize,
        any: bool,
    },
    /// A merged identity, or any ID minted so far if `any` is set.
    Reinstate {
        identity: usize,
        any: bool,
    },
    Constrain {
        cannot: bool,
        a: u128,
        b: u128,
    },
    /// A constraint in force, or the given one if `any` is set.
    Unconstrain {
        pick: usize,
        any: bool,
        cannot: bool,
        a: u128,
        b: u128,
    },
}

#[derive(Clone, Debug)]
enum From {
    /// Whatever the observation belongs to when the changeset is written.
    Current,
    Given(Option<usize>),
}

#[derive(Clone, Debug)]
enum To {
    /// An identity of the observation's entity type, if there is one.
    Compatible(usize),
    /// Any ID minted so far, whether or not it became an identity.
    Any(usize),
    Nothing,
}

fn op_spec() -> impl Strategy<Value = OpSpec> {
    let from = prop_oneof![
        6 => Just(From::Current),
        1 => proptest::option::of(0..8usize).prop_map(From::Given),
    ];
    let to = prop_oneof![
        8 => (0..8usize).prop_map(To::Compatible),
        1 => (0..8usize).prop_map(To::Any),
        1 => Just(To::Nothing),
    ];
    prop_oneof![
        1 => (proptest::option::weighted(0.1, 0..8usize), proptest::bool::weighted(0.25))
            .prop_map(|(reuse, gadget)| OpSpec::Create { reuse, gadget }),
        4 => (1..=OBSERVATIONS, from, to)
            .prop_map(|(observation, from, to)| OpSpec::Assign { observation, from, to }),
        2 => (0..8usize, 0..8usize, proptest::bool::weighted(0.15))
            .prop_map(|(from, into, any)| OpSpec::Merge { from, into, any }),
        2 => (0..8usize, proptest::bool::weighted(0.15))
            .prop_map(|(identity, any)| OpSpec::Reinstate { identity, any }),
        1 => (any::<bool>(), 1..=OBSERVATIONS, 1..=OBSERVATIONS)
            .prop_map(|(cannot, a, b)| OpSpec::Constrain { cannot, a, b }),
        1 => (0..8usize, proptest::bool::weighted(0.2), any::<bool>(), 1..=OBSERVATIONS, 1..=OBSERVATIONS)
            .prop_map(|(pick, any, cannot, a, b)| OpSpec::Unconstrain { pick, any, cannot, a, b }),
    ]
}

fn entity_type(gadget: bool) -> EntityType {
    EntityType::new(if gadget { "gadget" } else { "widget" }).unwrap()
}

fn is_gadget(observation: u128) -> bool {
    observation > 6
}

/// `a`, and an observation of the same entity type that is not `a`.
fn same_type(a: u128, b: u128) -> (u128, u128) {
    let (first, count) = if is_gadget(a) { (7, 2) } else { (1, 6) };
    let b = first + (b - 1) % count;
    let b = if b == a {
        first + (b - first + 1) % count
    } else {
        b
    };
    (a, b)
}

fn kind(cannot: bool) -> ConstraintKind {
    if cannot {
        ConstraintKind::CannotLink
    } else {
        ConstraintKind::MustLink
    }
}

fn pick<T: Copy>(from: &[T], index: usize) -> Option<T> {
    (!from.is_empty()).then(|| from[index % from.len()])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    #[test]
    fn memory_and_duckdb_agree(
        changesets in proptest::collection::vec(proptest::collection::vec(op_spec(), 1..6), 1..16)
    ) {
        // Observations 1 to 6 are widgets, 7 and 8 are gadgets.
        let observations: Vec<_> = (1..=OBSERVATIONS)
            .map(|n| (ObservationId::from_u128(n), entity_type(is_gadget(n))))
            .collect();
        let mut memory = MemoryState::new();
        for (id, entity_type) in &observations {
            memory.observe(*id, entity_type.clone());
        }
        let mut duck = DuckStore::open(None, Box::new(StepClock::new(0, 1))).unwrap();
        insert_observations(&duck.conn, &observations).unwrap();

        let mut ids = SeededIds::new(11);
        // Identities that exist, with whether each is a gadget, and every ID
        // minted whether or not its changeset was accepted.
        let mut existing: Vec<(IdentityId, bool)> = Vec::new();
        let mut minted: Vec<IdentityId> = Vec::new();

        for specs in changesets {
            let mut ops = Vec::new();
            // Identities that are active as the changeset is written, and new
            // ones as it makes them.
            let merged: Vec<IdentityId> = existing
                .iter()
                .map(|(id, _)| *id)
                .zip(memory.identities(&existing.iter().map(|(id, _)| *id).collect::<Vec<_>>()).unwrap())
                .filter(|(_, record)| {
                    record.as_ref().is_some_and(|record| record.status != IdentityStatus::Active)
                })
                .map(|(id, _)| id)
                .collect();
            let mut pending: Vec<(IdentityId, bool)> = existing
                .iter()
                .copied()
                .filter(|(id, _)| !merged.contains(id))
                .collect();
            let created_before = existing.clone();
            let mut created = Vec::new();
            let all: Vec<_> = observations.iter().map(|(id, _)| *id).collect();
            let in_force = memory.constraints_on(&all).unwrap();
            let mut moved = std::collections::HashSet::new();
            for spec in specs {
                match spec {
                    OpSpec::Create { reuse, gadget } => {
                        let reused = reuse.and_then(|index| pick(&minted, index));
                        let identity = reused.unwrap_or_else(|| ids.identity_id());
                        minted.push(identity);
                        pending.push((identity, gadget));
                        created.push((identity, gadget));
                        ops.push(Op::CreateIdentity { identity, entity_type: entity_type(gadget) });
                    }
                    OpSpec::Merge { from, into, any } => {
                        let (from, into) = if any {
                            (pick(&minted, from), pick(&minted, into))
                        } else {
                            let from = pick(&pending, from);
                            let same: Vec<IdentityId> = pending
                                .iter()
                                .filter(|(id, gadget)| {
                                    from.is_some_and(|(from, is_gadget)| *id != from && *gadget == is_gadget)
                                })
                                .map(|(id, _)| *id)
                                .collect();
                            (from.map(|(id, _)| id), pick(&same, into))
                        };
                        if let (Some(from), Some(into)) = (from, into) {
                            ops.push(Op::Merge { from, into });
                        }
                    }
                    OpSpec::Reinstate { identity, any } => {
                        let identity = if any { pick(&minted, identity) } else { pick(&merged, identity) };
                        if let Some(identity) = identity {
                            ops.push(Op::Reinstate { identity });
                        }
                    }
                    OpSpec::Constrain { cannot, a, b } => {
                        // Mostly two different observations of one type.
                        let (a, b) = if (a + b) % 5 == 0 { (a, b) } else { same_type(a, b) };
                        ops.push(Op::Constrain {
                            kind: kind(cannot),
                            a: ObservationId::from_u128(a),
                            b: ObservationId::from_u128(b),
                        });
                    }
                    OpSpec::Unconstrain { pick: index, any, cannot, a, b } => {
                        if any {
                            ops.push(Op::Unconstrain {
                                kind: kind(cannot),
                                a: ObservationId::from_u128(a),
                                b: ObservationId::from_u128(b),
                            });
                        } else if let Some(constraint) = pick(&in_force, index) {
                            ops.push(Op::Unconstrain {
                                kind: constraint.kind,
                                a: constraint.b,
                                b: constraint.a,
                            });
                        }
                    }
                    OpSpec::Assign { observation, from, to } => {
                        let gadget = is_gadget(observation);
                        let observation = ObservationId::from_u128(observation);
                        let current = memory.assignments(&[observation]).unwrap()[0];
                        // Mostly one move for an observation, and a move
                        // that goes somewhere else.
                        if matches!(from, From::Current) && !moved.insert(observation) {
                            continue;
                        }
                        let to = match to {
                            To::Compatible(index) => {
                                let compatible: Vec<IdentityId> = pending
                                    .iter()
                                    .filter(|(id, is_gadget)| {
                                        *is_gadget == gadget && Some(*id) != current
                                    })
                                    .map(|(id, _)| *id)
                                    .collect();
                                Some(pick(&compatible, index).unwrap_or_else(|| {
                                    let identity = ids.identity_id();
                                    minted.push(identity);
                                    pending.push((identity, gadget));
                                    created.push((identity, gadget));
                                    ops.push(Op::CreateIdentity {
                                        identity,
                                        entity_type: entity_type(gadget),
                                    });
                                    identity
                                }))
                            }
                            To::Any(index) => pick(&minted, index),
                            To::Nothing => None,
                        };
                        let from = match from {
                            From::Current => current,
                            From::Given(index) => index.and_then(|index| pick(&minted, index)),
                        };
                        ops.push(Op::Assign { observation, from, to });
                    }
                }
            }
            if ops.is_empty() {
                continue;
            }
            let changeset = Changeset {
                id: ids.changeset_id(),
                base: memory.head().unwrap(),
                actor: Actor::Engine,
                reason: None,
                run: None,
                ops,
            };

            let in_memory = validate(&memory, changeset.clone());
            let in_duck = validate(&duck, changeset);
            match (in_memory, in_duck) {
                (Ok(for_memory), Ok(for_duck)) => {
                    let a = memory.apply(for_memory).unwrap();
                    let b = duck.apply(for_duck).unwrap();
                    prop_assert_eq!(a, b);
                    existing = created_before;
                    existing.extend(created);
                }
                (Err(a), Err(b)) => prop_assert_eq!(a.rejections(), b.rejections()),
                (a, b) => prop_assert!(
                    false,
                    "memory said {:?} and DuckDB said {:?}",
                    a.map(|_| ()),
                    b.map(|_| ())
                ),
            }
        }

        let all: Vec<_> = observations.iter().map(|(id, _)| *id).collect();
        let identities = minted;
        prop_assert_eq!(memory.snapshot().unwrap(), duck.snapshot().unwrap());
        prop_assert_eq!(memory.log().unwrap(), duck.log().unwrap());
        prop_assert_eq!(memory.assignments(&all).unwrap(), duck.assignments(&all).unwrap());
        prop_assert_eq!(
            memory.identities(&identities).unwrap(),
            duck.identities(&identities).unwrap()
        );
        prop_assert_eq!(memory.members(&identities).unwrap(), duck.members(&identities).unwrap());
        prop_assert_eq!(
            memory.membership_changed(&identities).unwrap(),
            duck.membership_changed(&identities).unwrap()
        );
        prop_assert_eq!(memory.constraints_on(&all).unwrap(), duck.constraints_on(&all).unwrap());
        prop_assert_eq!(
            memory.constraints_on(&all[..2]).unwrap(),
            duck.constraints_on(&all[..2]).unwrap()
        );
        prop_assert_eq!(
            memory.constraints_of_members(&identities[..identities.len() / 2]).unwrap(),
            duck.constraints_of_members(&identities[..identities.len() / 2]).unwrap()
        );
        // Both give the same undo for every changeset, whether or not the
        // state still has room for it.
        for entry in memory.log().unwrap() {
            prop_assert_eq!(inverse(&memory, &entry).unwrap(), inverse(&duck, &entry).unwrap());
        }
        let replayed = MemoryState::replay(&duck.log().unwrap()).unwrap();
        prop_assert_eq!(replayed.snapshot().unwrap(), duck.snapshot().unwrap());
    }
}
