//! The operations that take back a committed changeset.

use std::collections::{HashMap, HashSet};

use crate::changeset::Op;
use crate::ids::ObservationId;
use crate::state::{IdentityState, LogEntry, Result, StateError};

/// The operations that undo a committed changeset, for the state as it is
/// now. They are not committed here: they go in a new changeset, which is
/// validated like any other, so an undo that no longer fits the state is
/// rejected and names the operation that does not fit.
///
/// Operations are undone last to first:
///
/// - an assignment is reversed;
/// - a constraint that was added is removed, and one that was removed is
///   added;
/// - a merge reinstates the merged identity and moves back the observations
///   the merge moved, leaving out any that have left the surviving identity
///   since;
/// - a reinstatement merges the identity back into the one it had been merged
///   into;
/// - creating an identity is not undone. Its ID is never reused, so the
///   identity stays, with whatever members the rest of the undo leaves it.
pub fn inverse<S: IdentityState + ?Sized>(state: &S, entry: &LogEntry) -> Result<Vec<Op>> {
    let ops = &entry.changeset.ops;
    let assigned: HashSet<ObservationId> = ops
        .iter()
        .filter_map(|op| match op {
            Op::Assign { observation, .. } => Some(*observation),
            _ => None,
        })
        .collect();

    let mut undo = Vec::new();
    for op in ops.iter().rev() {
        match op {
            Op::CreateIdentity { .. } => {}
            Op::Assign {
                observation,
                from,
                to,
            } => undo.push(Op::Assign {
                observation: *observation,
                from: *to,
                to: *from,
            }),
            Op::Constrain { kind, a, b } => undo.push(Op::Unconstrain {
                kind: *kind,
                a: *a,
                b: *b,
            }),
            Op::Unconstrain { kind, a, b } => undo.push(Op::Constrain {
                kind: *kind,
                a: *a,
                b: *b,
            }),
            Op::Merge { from, into } => {
                undo.push(Op::Reinstate { identity: *from });
                // An observation the same changeset assigned out of `from` is
                // put back by undoing that assignment.
                let moved: Vec<ObservationId> = state
                    .left_at(*from, entry.seq)?
                    .into_iter()
                    .filter(|observation| !assigned.contains(observation))
                    .collect();
                let now: HashMap<_, _> = moved
                    .iter()
                    .copied()
                    .zip(state.assignments(&moved)?)
                    .collect();
                undo.extend(
                    moved
                        .iter()
                        .filter(|observation| now[*observation] == Some(*into))
                        .map(|observation| Op::Assign {
                            observation: *observation,
                            from: Some(*into),
                            to: Some(*from),
                        }),
                );
            }
            Op::Reinstate { identity } => {
                let into = state.merged_before(*identity, entry.seq)?.ok_or_else(|| {
                    StateError::new(format!(
                        "the log has no merge of identity {identity} before position {}",
                        entry.seq
                    ))
                })?;
                undo.push(Op::Merge {
                    from: *identity,
                    into,
                });
            }
        }
    }
    Ok(undo)
}
