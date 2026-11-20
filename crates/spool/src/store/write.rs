//! Writing identity state: replacing the projection, and appending a
//! validated changeset's log rows and projection rows in one transaction.

use duckdb::params;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use spool_core::{Changeset, ContentHash, IdentityStatus, Seq, Snapshot};

use super::DuckStore;
use super::ops::op_rows;
use crate::batch::{Column, build_batch};
use crate::drafts;
use crate::error::Result;
use crate::possible_split;
use crate::review;

impl DuckStore {
    /// Replaces the projection tables with the given state.
    #[expect(clippy::too_many_lines)]
    pub(crate) fn write_projection(&mut self, snapshot: &Snapshot) -> Result<()> {
        let identities = build_batch(vec![
            (
                "identity_id",
                to_text_column(snapshot.identities.iter().map(|record| Some(record.id))),
            ),
            (
                "entity_type",
                to_text_column(
                    snapshot
                        .identities
                        .iter()
                        .map(|record| Some(&record.entity_type)),
                ),
            ),
            (
                "created_seq",
                Column::Integer(
                    snapshot
                        .identities
                        .iter()
                        .map(|record| record.created.0 as i64)
                        .collect(),
                ),
            ),
            (
                "merged_into",
                to_text_column(
                    snapshot
                        .identities
                        .iter()
                        .map(|record| match record.status {
                            IdentityStatus::Active => None,
                            IdentityStatus::MergedInto(survivor) => Some(survivor),
                        }),
                ),
            ),
            (
                "merged_seq",
                to_text_column(
                    snapshot
                        .identities
                        .iter()
                        .map(|record| record.merged.map(|merged| merged.seq.0)),
                ),
            ),
            (
                "merge_target",
                to_text_column(
                    snapshot
                        .identities
                        .iter()
                        .map(|record| record.merged.map(|merged| merged.into)),
                ),
            ),
        ])?;
        let memberships = build_batch(vec![
            (
                "record_id",
                to_text_column(snapshot.memberships.iter().map(|row| Some(row.record))),
            ),
            (
                "identity_id",
                to_text_column(snapshot.memberships.iter().map(|row| Some(row.identity))),
            ),
            (
                "from_seq",
                Column::Integer(
                    snapshot
                        .memberships
                        .iter()
                        .map(|row| row.from.0 as i64)
                        .collect(),
                ),
            ),
            (
                "to_seq",
                to_text_column(
                    snapshot
                        .memberships
                        .iter()
                        .map(|row| row.to.map(|seq| seq.0)),
                ),
            ),
        ])?;
        let links = &snapshot.constraints;
        let constraints = build_batch(vec![
            (
                "record_a",
                to_text_column(links.iter().map(|row| Some(row.constraint.a))),
            ),
            (
                "record_b",
                to_text_column(links.iter().map(|row| Some(row.constraint.b))),
            ),
            (
                "kind",
                to_text_column(links.iter().map(|row| Some(row.constraint.kind.as_str()))),
            ),
            (
                "from_seq",
                Column::Integer(links.iter().map(|row| row.from.0 as i64).collect()),
            ),
            (
                "to_seq",
                to_text_column(links.iter().map(|row| row.to.map(|seq| seq.0))),
            ),
        ])?;

        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM spool.identity; DELETE FROM spool.membership; DELETE FROM spool.constraint_link;",
        )?;
        tx.execute(
            "INSERT INTO spool.identity
             SELECT identity_id::UUID, entity_type, created_seq, merged_into::UUID,
                    merged_seq::BIGINT, merge_target::UUID
             FROM arrow(?)",
            [&ArrowBatchRegistration::new(identities)],
        )?;
        tx.execute(
            "INSERT INTO spool.membership
             SELECT record_id::UUID, identity_id::UUID, from_seq, to_seq::BIGINT FROM arrow(?)",
            [&ArrowBatchRegistration::new(memberships)],
        )?;
        tx.execute(
            "INSERT INTO spool.constraint_link
             SELECT record_a::UUID, record_b::UUID, kind, from_seq, to_seq::BIGINT
             FROM arrow(?)",
            [&ArrowBatchRegistration::new(constraints)],
        )?;
        tx.commit()?;
        Ok(())
    }

    #[expect(clippy::too_many_lines)]
    pub(super) fn append(
        &mut self,
        changeset: &Changeset,
        content_hash: &ContentHash,
    ) -> Result<Seq> {
        let committed_at = self.clock.now_micros();
        let ops = op_rows(&changeset.ops)?;

        let tx = self.conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT coalesce(max(seq), 0) + 1 FROM spool.changeset",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO spool.changeset
             VALUES (?, ?::UUID, ?, ?, ?, ?, ?, make_timestamptz(?))",
            params![
                seq,
                changeset.id.to_string(),
                changeset.base.0 as i64,
                changeset.actor.to_string(),
                changeset.reason,
                changeset.run.map(|run| run.0 as i64),
                content_hash.as_str(),
                committed_at,
            ],
        )?;
        tx.execute(
            "INSERT INTO spool.changeset_op
             SELECT ?, op_index::INTEGER, kind, identity_id::UUID, from_identity_id::UUID,
                    record_id::UUID, other_record_id::UUID, entity_type, constraint_kind
             FROM arrow(?)",
            params![seq, &ArrowBatchRegistration::new(ops)],
        )?;

        // The projection is derived from the log rows just written, one kind
        // of operation at a time. The validator's rules are what make that
        // equal to applying the operations in the order they were written: a
        // changeset moves a record once, changes an identity's status
        // once, and merges away no identity that gained members in it.
        let has = |kind: &str| changeset.ops.iter().any(|op| op.kind() == kind);
        tx.execute(
            "INSERT INTO spool.identity
             SELECT identity_id, entity_type, seq, NULL, NULL, NULL
             FROM spool.changeset_op WHERE seq = ? AND kind = 'create_identity'",
            [seq],
        )?;
        if has("reinstate") {
            tx.execute(
                "UPDATE spool.identity
                 SET merged_into = NULL, merged_seq = NULL, merge_target = NULL
                 WHERE identity_id IN (
                   SELECT identity_id FROM spool.changeset_op WHERE seq = ? AND kind = 'reinstate')",
                [seq],
            )?;
        }
        tx.execute(
            "UPDATE spool.membership SET to_seq = ?
             WHERE to_seq IS NULL AND record_id IN (
               SELECT record_id FROM spool.changeset_op
               WHERE seq = ? AND kind = 'assign' AND from_identity_id IS NOT NULL)",
            [seq, seq],
        )?;
        tx.execute(
            "INSERT INTO spool.membership
             SELECT record_id, identity_id, seq, NULL
             FROM spool.changeset_op
             WHERE seq = ? AND kind = 'assign' AND identity_id IS NOT NULL",
            [seq],
        )?;
        if has("merge") {
            // What is left in a merged identity moves to the survivor, which
            // is never itself merged away by the same changeset.
            tx.execute(
                "INSERT INTO spool.membership
                 SELECT m.record_id, op.identity_id, op.seq, NULL
                 FROM spool.changeset_op op
                 JOIN spool.membership m
                   ON m.identity_id = op.from_identity_id AND m.to_seq IS NULL
                 WHERE op.seq = ? AND op.kind = 'merge'",
                [seq],
            )?;
            tx.execute(
                "UPDATE spool.membership SET to_seq = ?
                 WHERE to_seq IS NULL AND identity_id IN (
                   SELECT from_identity_id FROM spool.changeset_op
                   WHERE seq = ? AND kind = 'merge')",
                [seq, seq],
            )?;
            // Identities that stood for the merged one stand for the survivor.
            tx.execute(
                "UPDATE spool.identity AS i SET merged_into = op.identity_id
                 FROM spool.changeset_op op
                 WHERE op.seq = ? AND op.kind = 'merge' AND i.merged_into = op.from_identity_id",
                [seq],
            )?;
            tx.execute(
                "UPDATE spool.identity AS i
                 SET merged_into = op.identity_id, merged_seq = op.seq,
                     merge_target = op.identity_id
                 FROM spool.changeset_op op
                 WHERE op.seq = ? AND op.kind = 'merge' AND i.identity_id = op.from_identity_id",
                [seq],
            )?;
        }
        if has("reinstate") {
            // Identities merged into a reinstated one, directly or through
            // others, stand for it again.
            tx.execute(
                "UPDATE spool.identity AS i SET merged_into = under.root
                 FROM (
                   WITH RECURSIVE under(root, identity_id) AS (
                     SELECT op.identity_id, child.identity_id
                     FROM spool.changeset_op op
                     JOIN spool.identity child ON child.merge_target = op.identity_id
                     WHERE op.seq = ? AND op.kind = 'reinstate'
                     UNION ALL
                     SELECT under.root, child.identity_id
                     FROM under
                     JOIN spool.identity child ON child.merge_target = under.identity_id
                   )
                   SELECT root, identity_id FROM under
                 ) under
                 WHERE i.identity_id = under.identity_id",
                [seq],
            )?;
        }
        if has("unconstrain") {
            tx.execute(
                "UPDATE spool.constraint_link AS c SET to_seq = op.seq
                 FROM spool.changeset_op op
                 WHERE op.seq = ? AND op.kind = 'unconstrain' AND c.to_seq IS NULL
                   AND c.kind = op.constraint_kind
                   AND c.record_a = least(op.record_id, op.other_record_id)
                   AND c.record_b = greatest(op.record_id, op.other_record_id)",
                [seq],
            )?;
        }
        if has("constrain") {
            tx.execute(
                "INSERT INTO spool.constraint_link
                 SELECT least(record_id, other_record_id),
                        greatest(record_id, other_record_id),
                        constraint_kind, seq, NULL
                 FROM spool.changeset_op WHERE seq = ? AND kind = 'constrain'",
                [seq],
            )?;
        }
        // Not identity state, but it has to agree with membership at every
        // log position, so it is updated in the same transaction.
        review::resolve_assigned(&tx, Seq(seq as u64))?;
        possible_split::resolve_answered(&tx, Seq(seq as u64))?;
        // Nor is a draft, and committing one is what ends its match run.
        drafts::mark_committed(&tx, changeset.id, Seq(seq as u64), committed_at)?;
        tx.commit()?;
        Ok(Seq(seq as u64))
    }
}

fn to_text_column<T: ToString>(values: impl Iterator<Item = Option<T>>) -> Column {
    Column::Text(
        values
            .map(|value| value.map(|value| value.to_string()))
            .collect(),
    )
}
