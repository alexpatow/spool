//! Random changesets, valid and invalid, must have the same outcome in the
//! in-memory state and in the DuckDB store.

use proptest::prelude::*;
use spool_core::{
    Actor, Changeset, EntityType, IdGenerator, IdentityId, IdentityState, IdentityStore,
    MemoryState, ObservationId, Op, SeededIds, StepClock, validate,
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
        3 => (1..=OBSERVATIONS, from, to)
            .prop_map(|(observation, from, to)| OpSpec::Assign { observation, from, to }),
    ]
}

fn entity_type(gadget: bool) -> EntityType {
    EntityType::new(if gadget { "gadget" } else { "widget" }).unwrap()
}

fn is_gadget(observation: u128) -> bool {
    observation > 6
}

fn pick<T: Copy>(from: &[T], index: usize) -> Option<T> {
    (!from.is_empty()).then(|| from[index % from.len()])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn memory_and_duckdb_agree(
        changesets in proptest::collection::vec(proptest::collection::vec(op_spec(), 1..5), 1..14)
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
            let mut pending = existing.clone();
            for spec in specs {
                match spec {
                    OpSpec::Create { reuse, gadget } => {
                        let reused = reuse.and_then(|index| pick(&minted, index));
                        let identity = reused.unwrap_or_else(|| ids.identity_id());
                        minted.push(identity);
                        pending.push((identity, gadget));
                        ops.push(Op::CreateIdentity { identity, entity_type: entity_type(gadget) });
                    }
                    OpSpec::Assign { observation, from, to } => {
                        let gadget = is_gadget(observation);
                        let observation = ObservationId::from_u128(observation);
                        let to = match to {
                            To::Compatible(index) => {
                                let compatible: Vec<IdentityId> = pending
                                    .iter()
                                    .filter(|(_, is_gadget)| *is_gadget == gadget)
                                    .map(|(id, _)| *id)
                                    .collect();
                                Some(pick(&compatible, index).unwrap_or_else(|| {
                                    let identity = ids.identity_id();
                                    minted.push(identity);
                                    pending.push((identity, gadget));
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
                            From::Current => memory.assignments(&[observation]).unwrap()[0],
                            From::Given(index) => index.and_then(|index| pick(&minted, index)),
                        };
                        ops.push(Op::Assign { observation, from, to });
                    }
                }
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
                    existing = pending;
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
        let replayed = MemoryState::replay(&duck.log().unwrap()).unwrap();
        prop_assert_eq!(replayed.snapshot().unwrap(), duck.snapshot().unwrap());
    }
}
