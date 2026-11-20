//! The corrections a reviewer would make, and whether what they asked for
//! still holds.

use std::collections::{BTreeSet, HashMap, HashSet};

use spool::core::{Actor, Changeset, Constraint, ConstraintKind, IdentityId, Op, Seq};
use spool::{ConstraintFilter, Engine, RecordEntry, RecordFilter};

use crate::error::{Error, Result};
use crate::metrics::{OverridePersistence, SplitRecovery};
use crate::report::CorrectionSummary;
use crate::score::grouping;
use crate::spec::{Correction, CorrectionSpec};

/// What the corrections so far have done that later steps are measured
/// against.
#[derive(Default)]
pub(crate) struct Tracked {
    /// Named corrections: where each was committed, and the split it made.
    named: HashMap<String, (Seq, Option<usize>)>,
    /// The two sides of each split, as record IDs. `None` once undone.
    splits: Vec<Option<[Vec<String>; 2]>>,
    constraints: BTreeSet<Constraint>,
}

impl Tracked {
    /// Follows the constraints a committed changeset recorded and removed.
    fn follow(&mut self, changeset: &Changeset) {
        for op in &changeset.ops {
            match op {
                Op::Constrain { kind, a, b } => {
                    self.constraints.insert(Constraint::new(*kind, *a, *b));
                }
                Op::Unconstrain { kind, a, b } => {
                    self.constraints.remove(&Constraint::new(*kind, *a, *b));
                }
                _ => {}
            }
        }
    }
}

/// The version of a row the source holds now, named as `source:key`.
fn record(engine: &Engine, reference: &str) -> Result<RecordEntry> {
    let (source, key) = reference
        .split_once(':')
        .expect("checked when the scenario was loaded");
    engine
        .records(&RecordFilter::keyed(source, key))?
        .into_iter()
        .find(|version| version.superseded_by.is_none())
        .ok_or_else(|| Error::Scenario(format!("no record is `{reference}`")))
}

fn identity_of(engine: &Engine, reference: &str) -> Result<IdentityId> {
    record(engine, reference)?
        .identity
        .ok_or_else(|| Error::Scenario(format!("`{reference}` has no identity")))
}

fn members(engine: &Engine, identity: IdentityId) -> Result<Vec<String>> {
    Ok(engine
        .identity(identity)?
        .members
        .iter()
        .map(|member| member.record.to_string())
        .collect())
}

/// Applies one correction as a reviewer would: through the calls any caller
/// uses, as the user `benchmark`.
pub(crate) fn correct(
    engine: &Engine,
    step: &str,
    spec: &CorrectionSpec,
    tracked: &mut Tracked,
) -> Result<CorrectionSummary> {
    let actor = Actor::User("benchmark".to_owned());
    let reason = spec
        .reason
        .clone()
        .or_else(|| spec.name.clone())
        .or_else(|| Some(step.to_owned()));
    let kind = spec.kind().expect("checked when the scenario was loaded");
    let author = |ops: Vec<Op>| engine.author(actor.clone(), reason.clone(), ops);

    let mut split = None;
    let (changeset, seq) = match &kind {
        Correction::Merge([from, into]) => {
            let authored = author(vec![Op::Merge {
                from: identity_of(engine, from)?,
                into: identity_of(engine, into)?,
            }])?;
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
        Correction::Assign([moved, target]) => {
            let moved = record(engine, moved)?;
            let authored = author(vec![Op::Assign {
                record: moved.id,
                from: moved.identity,
                to: Some(identity_of(engine, target)?),
            }])?;
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
        Correction::Split(records) => {
            let identity = identity_of(engine, &records[0])?;
            let records = records
                .iter()
                .map(|reference| record(engine, reference).map(|found| found.id))
                .collect::<Result<Vec<_>>>()?;
            let report = engine.split(
                identity,
                &records,
                true,
                None,
                actor.clone(),
                reason.clone(),
            )?;
            tracked.splits.push(Some([
                members(engine, report.kept)?,
                members(engine, report.identity)?,
            ]));
            split = Some(tracked.splits.len() - 1);
            (report.changeset, Some(report.committed.seq))
        }
        Correction::Constrain(kind, [a, b]) => {
            let authored = author(vec![Op::Constrain {
                kind: *kind,
                a: record(engine, a)?.id,
                b: record(engine, b)?.id,
            }])?;
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
        Correction::Undo(name) => {
            let (seq, split) = tracked.named[*name];
            let authored = engine.undo(seq, actor.clone(), reason.clone(), true)?;
            if let Some(split) = split {
                tracked.splits[split] = None;
            }
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
    };
    let seq = seq.expect("the changeset was committed");
    tracked.follow(&changeset);
    if let Some(name) = &spec.name {
        tracked.named.insert(name.clone(), (seq, split));
    }
    Ok(CorrectionSummary {
        name: spec.name.clone(),
        kind: kind.name().to_owned(),
        seq,
    })
}

/// Measures what the scenario's corrections asked for against current state.
pub(crate) fn held(
    engine: &Engine,
    tracked: &Tracked,
) -> Result<(SplitRecovery, OverridePersistence)> {
    let splits = tracked.splits.iter().flatten().count() as u64;
    let constraints = tracked.constraints.len() as u64;
    if splits == 0 && constraints == 0 {
        return Ok((SplitRecovery::new(0, 0), OverridePersistence::new(0, 0)));
    }
    let groups: HashMap<String, String> = grouping(engine)?.into_iter().collect();
    let group = |record: &String| groups.get(record);

    let kept_apart = tracked
        .splits
        .iter()
        .flatten()
        .filter(|[kept, moved]| {
            let kept: HashSet<_> = kept.iter().filter_map(group).collect();
            !moved
                .iter()
                .filter_map(group)
                .any(|group| kept.contains(group))
        })
        .count() as u64;

    let in_force: HashSet<Constraint> = engine
        .constraints(&ConstraintFilter::default())?
        .into_iter()
        .map(|record| record.constraint)
        .collect();
    let honoured = tracked
        .constraints
        .iter()
        .filter(|constraint| {
            let together = group(&constraint.a.to_string()) == group(&constraint.b.to_string());
            in_force.contains(constraint)
                && match constraint.kind {
                    ConstraintKind::MustLink => together,
                    ConstraintKind::CannotLink => !together,
                }
        })
        .count() as u64;

    Ok((
        SplitRecovery::new(splits, kept_apart),
        OverridePersistence::new(constraints, honoured),
    ))
}
