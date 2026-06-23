use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::oneshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Selected { option_id: String },
    Cancelled,
}

struct PendingPermission {
    session_id: String,
    sender: oneshot::Sender<PermissionDecision>,
}

#[derive(Default)]
pub struct PermissionRegistry {
    pending: Mutex<HashMap<String, PendingPermission>>,
}

impl PermissionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        request_id: String,
        session_id: String,
    ) -> oneshot::Receiver<PermissionDecision> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(request_id, PendingPermission { session_id, sender });
        receiver
    }

    pub fn respond(&self, request_id: &str, decision: PermissionDecision) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .unwrap()
            .remove(request_id)
            .ok_or_else(|| "Permission request is no longer pending".to_string())?;
        pending
            .sender
            .send(decision)
            .map_err(|_| "Permission request is no longer waiting for a response".to_string())
    }

    pub fn cancel_session(&self, session_id: &str) {
        let mut pending = self.pending.lock().unwrap();
        let request_ids: Vec<String> = pending
            .iter()
            .filter_map(|(request_id, pending)| {
                (pending.session_id == session_id).then(|| request_id.clone())
            })
            .collect();

        for request_id in request_ids {
            if let Some(pending) = pending.remove(&request_id) {
                let _ = pending.sender.send(PermissionDecision::Cancelled);
            }
        }
    }

    pub fn unregister(&self, request_id: &str) {
        self.pending.lock().unwrap().remove(request_id);
    }
}
