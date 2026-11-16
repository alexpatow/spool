//! The in-memory state passes the conformance suite.

use spool_core::{EntityType, MemoryState, ObservationId, conformance_tests};

fn store(observations: &[(ObservationId, EntityType)]) -> MemoryState {
    let mut state = MemoryState::new();
    for (id, entity_type) in observations {
        state.observe(*id, entity_type.clone());
    }
    state
}

conformance_tests!(store);
