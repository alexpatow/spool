//! Injected sources of new IDs and of the current time.
//!
//! Nothing in the engine mints an ID or reads a clock except through these
//! traits, so a test that injects [`SeededIds`] and [`StepClock`] is
//! reproducible.

use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ids::{ChangesetId, IdentityId};

pub trait IdGenerator {
    fn identity_id(&mut self) -> IdentityId;
    fn changeset_id(&mut self) -> ChangesetId;
}

/// UUIDv7 from the system clock and the system's random source.
#[derive(Clone, Copy, Debug, Default)]
pub struct RandomIds;

impl IdGenerator for RandomIds {
    fn identity_id(&mut self) -> IdentityId {
        IdentityId::from_uuid(Uuid::now_v7())
    }

    fn changeset_id(&mut self) -> ChangesetId {
        ChangesetId::from_uuid(Uuid::now_v7())
    }
}

/// A reproducible sequence of UUIDv7 values. The same seed gives the same
/// sequence, so it must not be used twice against one store: the second use
/// would mint IDs the store already holds, and those changesets are rejected.
#[derive(Clone, Debug)]
pub struct SeededIds {
    seed: u64,
    counter: u64,
}

impl SeededIds {
    pub fn new(seed: u64) -> Self {
        Self { seed, counter: 0 }
    }

    fn next(&mut self) -> Uuid {
        self.counter += 1;
        let mut hasher = Sha256::new();
        hasher.update(self.seed.to_le_bytes());
        hasher.update(self.counter.to_le_bytes());
        let digest = hasher.finalize();
        let mut random = [0u8; 10];
        random.copy_from_slice(&digest[..10]);
        // The counter stands in for the timestamp, which keeps the sequence sorted.
        uuid::Builder::from_unix_timestamp_millis(self.counter, &random).into_uuid()
    }
}

impl IdGenerator for SeededIds {
    fn identity_id(&mut self) -> IdentityId {
        IdentityId::from_uuid(self.next())
    }

    fn changeset_id(&mut self) -> ChangesetId {
        ChangesetId::from_uuid(self.next())
    }
}

pub trait Clock {
    /// Microseconds since the Unix epoch.
    fn now_micros(&mut self) -> i64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_micros(&mut self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as i64)
            .unwrap_or(0)
    }
}

/// A clock that starts at a fixed time and advances by a fixed step on every
/// reading.
#[derive(Clone, Debug)]
pub struct StepClock {
    next: i64,
    step: i64,
}

impl StepClock {
    pub fn new(start_micros: i64, step_micros: i64) -> Self {
        Self {
            next: start_micros,
            step: step_micros,
        }
    }
}

impl Clock for StepClock {
    fn now_micros(&mut self) -> i64 {
        let now = self.next;
        self.next += self.step;
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_ids_repeat_for_a_seed_and_differ_between_seeds() {
        let take = |seed| {
            let mut ids = SeededIds::new(seed);
            (0..5).map(|_| ids.identity_id()).collect::<Vec<_>>()
        };
        assert_eq!(take(7), take(7));
        assert_ne!(take(7), take(8));
    }

    #[test]
    fn seeded_ids_are_distinct_sorted_version_7() {
        let mut ids = SeededIds::new(1);
        let minted: Vec<_> = (0..100).map(|_| ids.identity_id()).collect();
        let mut sorted = minted.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(minted, sorted);
        assert!(minted.iter().all(|id| id.as_uuid().get_version_num() == 7));
    }

    #[test]
    fn random_ids_are_version_7() {
        assert_eq!(RandomIds.identity_id().as_uuid().get_version_num(), 7);
    }

    #[test]
    fn step_clock_advances() {
        let mut clock = StepClock::new(10, 5);
        assert_eq!(
            [clock.now_micros(), clock.now_micros(), clock.now_micros()],
            [10, 15, 20]
        );
    }
}
