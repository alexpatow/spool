//! The in-memory state passes the conformance suite.

use spool_core::{EntityType, MemoryState, RecordId, conformance_tests};

fn store(records: &[(RecordId, EntityType)]) -> MemoryState {
    let mut state = MemoryState::new();
    for (id, entity_type) in records {
        state.add_record(*id, entity_type.clone());
    }
    state
}

conformance_tests!(store);
