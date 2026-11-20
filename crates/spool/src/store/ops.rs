//! Operations as rows of `spool.changeset_op`, and back.

use std::str::FromStr;

use spool_core::{ConstraintKind, EntityType, Op, StateError};

use super::parse_stored;
use crate::batch::{Column, build_batch};
use crate::error::Result;

pub(super) fn constraint_kind(value: Option<&str>) -> Result<ConstraintKind> {
    match value {
        Some("must_link") => Ok(ConstraintKind::MustLink),
        Some("cannot_link") => Ok(ConstraintKind::CannotLink),
        other => Err(StateError::new(format!("unknown constraint kind {other:?}")).into()),
    }
}

/// One row of `spool.changeset_op`, IDs as text.
#[derive(Default)]
pub(crate) struct OpRow {
    pub(crate) kind: String,
    pub(crate) identity: Option<String>,
    pub(crate) from_identity: Option<String>,
    pub(crate) record: Option<String>,
    pub(crate) other_record: Option<String>,
    pub(crate) entity_type: Option<String>,
    pub(crate) constraint_kind: Option<String>,
}

fn op_to_row(op: &Op) -> OpRow {
    let text = |value: &dyn ToString| Some(value.to_string());
    let row = OpRow {
        kind: op.kind().to_owned(),
        ..OpRow::default()
    };
    match op {
        Op::CreateIdentity {
            identity,
            entity_type,
        } => OpRow {
            identity: text(identity),
            entity_type: text(entity_type),
            ..row
        },
        Op::Assign { record, from, to } => OpRow {
            record: text(record),
            from_identity: from.map(|id| id.to_string()),
            identity: to.map(|id| id.to_string()),
            ..row
        },
        Op::Merge { from, into } => OpRow {
            from_identity: text(from),
            identity: text(into),
            ..row
        },
        Op::Constrain { kind, a, b } | Op::Unconstrain { kind, a, b } => OpRow {
            record: text(a),
            other_record: text(b),
            constraint_kind: Some(kind.as_str().to_owned()),
            ..row
        },
        Op::Reinstate { identity } => OpRow {
            identity: text(identity),
            ..row
        },
    }
}

pub(crate) fn op_from_row(row: OpRow) -> Result<Op> {
    fn parse_required<T>(value: Option<String>, column: &str) -> Result<T>
    where
        T: FromStr,
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        let value = value.ok_or_else(|| {
            StateError::new(format!("an operation row has no value in `{column}`"))
        })?;
        parse_stored(&value)
    }
    fn parse_optional<T>(value: Option<String>) -> Result<Option<T>>
    where
        T: FromStr,
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        value.as_deref().map(parse_stored).transpose()
    }
    let constraint_kind = |value: Option<String>| constraint_kind(value.as_deref());

    Ok(match row.kind.as_str() {
        "create_identity" => Op::CreateIdentity {
            identity: parse_required(row.identity, "identity_id")?,
            entity_type: EntityType::new(row.entity_type.unwrap_or_default())
                .map_err(StateError::new)?,
        },
        "assign" => Op::Assign {
            record: parse_required(row.record, "record_id")?,
            from: parse_optional(row.from_identity)?,
            to: parse_optional(row.identity)?,
        },
        "merge" => Op::Merge {
            from: parse_required(row.from_identity, "from_identity_id")?,
            into: parse_required(row.identity, "identity_id")?,
        },
        "constrain" => Op::Constrain {
            kind: constraint_kind(row.constraint_kind)?,
            a: parse_required(row.record, "record_id")?,
            b: parse_required(row.other_record, "other_record_id")?,
        },
        "unconstrain" => Op::Unconstrain {
            kind: constraint_kind(row.constraint_kind)?,
            a: parse_required(row.record, "record_id")?,
            b: parse_required(row.other_record, "other_record_id")?,
        },
        "reinstate" => Op::Reinstate {
            identity: parse_required(row.identity, "identity_id")?,
        },
        other => {
            return Err(
                StateError::new(format!("unknown operation kind {other:?} in the log")).into(),
            );
        }
    })
}

pub(crate) fn op_rows(ops: &[Op]) -> Result<duckdb::arrow::record_batch::RecordBatch> {
    let rows: Vec<OpRow> = ops.iter().map(op_to_row).collect();
    let column = |pick: fn(&OpRow) -> Option<String>| Column::Text(rows.iter().map(pick).collect());
    build_batch(vec![
        (
            "op_index",
            Column::Integer((0..rows.len() as i64).collect()),
        ),
        ("kind", column(|row| Some(row.kind.clone()))),
        ("identity_id", column(|row| row.identity.clone())),
        ("from_identity_id", column(|row| row.from_identity.clone())),
        ("record_id", column(|row| row.record.clone())),
        ("other_record_id", column(|row| row.other_record.clone())),
        ("entity_type", column(|row| row.entity_type.clone())),
        ("constraint_kind", column(|row| row.constraint_kind.clone())),
    ])
}

#[cfg(test)]
mod tests {
    use spool_core::RecordId;

    use super::*;

    #[test]
    fn every_operation_kind_survives_the_log_row_format() {
        let mut ids = spool_core::SeededIds::new(5);
        use spool_core::IdGenerator;
        let (x, y) = (ids.identity_id(), ids.identity_id());
        let (a, b) = (RecordId::from_u128(1), RecordId::from_u128(2));
        let ops = [
            Op::CreateIdentity {
                identity: x,
                entity_type: EntityType::new("widget").unwrap(),
            },
            Op::Assign {
                record: a,
                from: None,
                to: Some(x),
            },
            Op::Assign {
                record: a,
                from: Some(x),
                to: Some(y),
            },
            Op::Assign {
                record: a,
                from: Some(y),
                to: None,
            },
            Op::Merge { from: x, into: y },
            Op::Constrain {
                kind: ConstraintKind::CannotLink,
                a,
                b,
            },
            Op::Unconstrain {
                kind: ConstraintKind::MustLink,
                a,
                b,
            },
            Op::Reinstate { identity: x },
        ];
        for op in ops {
            assert_eq!(op_from_row(op_to_row(&op)).unwrap(), op);
        }
    }
}
