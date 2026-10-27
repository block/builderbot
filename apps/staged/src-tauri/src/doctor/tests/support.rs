//! Fixtures shared by the doctor test modules.

use crate::managed_acp_tools::ReconcileRecord;

pub(super) fn pinned_version() -> String {
    crate::managed_node::node_runtime_lock().version.clone()
}

pub(super) fn reconcile_record(ok: bool, errors: Vec<&str>) -> ReconcileRecord {
    ReconcileRecord {
        at_ms: 1_000,
        ok,
        errors: errors.into_iter().map(str::to_string).collect(),
    }
}
