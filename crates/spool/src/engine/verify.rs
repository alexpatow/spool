//! Verifying: checking the projection tables against a replay of the log
//! and against what the validator guarantees, and rebuilding them.

use spool_core::{IdentityStore, MemoryState};

use super::{Engine, Inner};
use crate::error::{Error, Result};
use crate::possible_split;
use crate::retract;
use crate::review;

/// What the store must never hold, each with the SQL that counts it.
const CHECKS: &[(&str, &str)] = &[
    (
        "record IDs that occur more than once",
        "SELECT count(*) FROM (
           SELECT record_id FROM spool.record GROUP BY 1 HAVING count(*) > 1)",
    ),
    (
        "changeset IDs that occur more than once",
        "SELECT count(*) FROM (
           SELECT changeset_id FROM spool.changeset GROUP BY 1 HAVING count(*) > 1)",
    ),
    (
        "records that belong to more than one identity",
        "SELECT count(*) FROM (
           SELECT record_id FROM spool.membership WHERE to_seq IS NULL
           GROUP BY 1 HAVING count(*) > 1)",
    ),
    (
        "memberships of records the store does not hold",
        "SELECT count(*) FROM spool.membership m
         WHERE NOT EXISTS (
           SELECT 1 FROM spool.record o WHERE o.record_id = m.record_id)",
    ),
    (
        "current memberships in an identity that is missing, merged, or of another entity type",
        "SELECT count(*) FROM spool.membership m
         JOIN spool.record o ON o.record_id = m.record_id
         LEFT JOIN spool.identity i ON i.identity_id = m.identity_id
         WHERE m.to_seq IS NULL
           AND (i.identity_id IS NULL OR i.merged_into IS NOT NULL
                OR i.entity_type <> o.entity_type)",
    ),
    (
        "`possible_split` review items that do not name one identity and two groups of its records, or review items of another kind that have groups",
        "SELECT count(*) FROM spool.review_item i
         WHERE CASE WHEN i.kind = 'possible_split'
           THEN len(i.identity_ids) <> 1 OR i.groups IS NULL OR len(i.groups) < 2
                OR list_sort(flatten(i.groups)) <> list_sort(i.record_ids)
           ELSE i.groups IS NOT NULL END",
    ),
    (
        "records that were taken back and belong to an identity",
        "SELECT count(*) FROM spool.retraction t
         JOIN spool.membership m ON m.record_id = t.record_id AND m.to_seq IS NULL
         WHERE t.restored_run IS NULL",
    ),
    (
        "records that are taken back more than once",
        "SELECT count(*) FROM (
           SELECT record_id FROM spool.retraction WHERE restored_run IS NULL
           GROUP BY 1 HAVING count(*) > 1)",
    ),
    (
        "retractions of records the store does not hold",
        "SELECT count(*) FROM spool.retraction t
         WHERE NOT EXISTS (
           SELECT 1 FROM spool.record o WHERE o.record_id = t.record_id)",
    ),
    (
        "retractions that name a changeset that did not take the record out of an identity",
        "SELECT count(*) FROM spool.retraction t
         WHERE t.detached_seq IS NOT NULL AND NOT EXISTS (
           SELECT 1 FROM spool.membership m
           WHERE m.record_id = t.record_id AND m.to_seq = t.detached_seq)",
    ),
    (
        "open drafts that place a record that was taken back",
        "SELECT count(*) FROM spool.draft d
         WHERE d.status = 'open' AND EXISTS (
           SELECT 1 FROM spool.draft_op o
           JOIN spool.retraction t
             ON t.record_id = o.record_id AND t.restored_run IS NULL
           WHERE o.changeset_id = d.changeset_id AND o.kind = 'assign'
             AND o.identity_id IS NOT NULL)",
    ),
    (
        "merged identities that stand for an identity that is missing or merged",
        "SELECT count(*) FROM spool.identity i
         LEFT JOIN spool.identity t ON t.identity_id = i.merged_into
         WHERE i.merged_into IS NOT NULL
           AND (t.identity_id IS NULL OR t.merged_into IS NOT NULL
                OR t.entity_type <> i.entity_type)",
    ),
    (
        "constraints in force that current membership breaks",
        "SELECT count(*) FROM spool.constraint_link c
         JOIN spool.membership a ON a.record_id = c.record_a AND a.to_seq IS NULL
         JOIN spool.membership b ON b.record_id = c.record_b AND b.to_seq IS NULL
         WHERE c.to_seq IS NULL
           AND (a.identity_id = b.identity_id) = (c.kind = 'cannot_link')",
    ),
    (
        "pairs of records with more than one constraint in force",
        "SELECT count(*) FROM (
           SELECT record_a, record_b FROM spool.constraint_link
           WHERE to_seq IS NULL GROUP BY 1, 2 HAVING count(*) > 1)",
    ),
    (
        "records superseded by more than one record",
        "SELECT count(*) FROM (
           SELECT supersedes FROM spool.record WHERE supersedes IS NOT NULL
           GROUP BY 1 HAVING count(*) > 1)",
    ),
    (
        "entity types with more than one open draft",
        "SELECT count(*) FROM (
           SELECT entity_type FROM spool.draft WHERE status = 'open'
           GROUP BY 1 HAVING count(*) > 1)",
    ),
    (
        "open drafts whose match run is not waiting on them",
        "SELECT count(*) FROM spool.draft d
         LEFT JOIN spool.run r ON r.run_id = d.match_run
         WHERE d.status = 'open' AND r.status IS DISTINCT FROM 'drafted'",
    ),
    (
        "committed drafts that are not in the log",
        "SELECT count(*) FROM spool.draft d
         WHERE d.status = 'committed' AND NOT EXISTS (
           SELECT 1 FROM spool.changeset c
           WHERE c.seq = d.closed_seq AND c.changeset_id = d.changeset_id)",
    ),
];

impl Engine {
    /// Checks that the projection tables equal a replay of the log, and that
    /// the tables hold what the validator guarantees.
    pub fn verify(&self) -> Result<()> {
        self.lock().verify()
    }

    /// Replaces the projection tables with a replay of the log.
    pub fn rebuild_projection(&self) -> Result<()> {
        let mut inner = self.lock();
        let replayed = MemoryState::replay(&inner.store.log()?)?;
        inner.store.write_projection(&replayed.snapshot()?)
    }
}

impl Inner {
    fn verify(&mut self) -> Result<()> {
        let mut problems = Vec::new();

        let replayed = MemoryState::replay(&self.store.log()?)?;
        if replayed.snapshot()? != self.store.snapshot()? {
            problems.push("the projection tables differ from a replay of the log".to_owned());
        }

        let mut checks: Vec<(String, String)> = CHECKS
            .iter()
            .map(|(what, sql)| ((*what).to_owned(), (*sql).to_owned()))
            .collect();
        checks.push((
            "open review items none of whose records waits for an identity".to_owned(),
            format!(
                "SELECT count(*) FROM spool.review_item i
                 WHERE i.status = 'open' AND {} AND i.review_id NOT IN ({})",
                review::ABOUT_WAITING,
                retract::WAITING
            ),
        ));
        let mut statement = self
            .store
            .conn
            .prepare("SELECT DISTINCT entity_type FROM spool.entity_type ORDER BY 1")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let entity_type: String = row.get(0)?;
            checks.push((
                format!("`{entity_type}` records with more than one feature row"),
                format!(
                    "SELECT count(*) FROM (
                       SELECT record_id FROM spool.features_{entity_type}
                       GROUP BY 1 HAVING count(*) > 1)"
                ),
            ));
            checks.push((
                format!("`{entity_type}` records that were taken back and have a feature row"),
                format!(
                    "SELECT count(*) FROM spool.features_{entity_type} f
                     JOIN spool.retraction t
                       ON t.record_id = f.record_id AND t.restored_run IS NULL"
                ),
            ));
        }
        drop(rows);
        drop(statement);

        for (what, sql) in checks {
            let found = self.count(&sql)?;
            if found > 0 {
                problems.push(format!("{found} {what}"));
            }
        }
        // An item is closed in the transaction that answers it, so none
        // stays open once what the store knows holds its groups together or
        // they no longer share the identity, and an identity has one at
        // most.
        let stale = possible_split::count_stale_items(&self.store.conn)?;
        if stale > 0 {
            problems.push(format!(
                "{stale} open `possible_split` review items with nothing left to ask, or that ask what another asks"
            ));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Verify(problems))
        }
    }
}
