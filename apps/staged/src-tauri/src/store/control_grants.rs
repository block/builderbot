//! Durable class B control grants (`control_grants`): the outside clients
//! the user approved in the consent dialog, each identified by a reconnect
//! secret whose SHA-256 is the only thing stored here.
//!
//! The registry in `control_server::grants` holds the live bearers; this
//! table is what survives an app restart so `stagedctl connect` is asked
//! once per client, not once per launch. A successful reconnect renews the
//! row for another [`CONTROL_GRANT_LIFETIME_MS`] from now — never further —
//! and records `last_used_at`; a revoke stamps `revoked_at` and the row
//! stays as the audit record. None of these publish a [`StoreChange`]: the
//! Settings panel refreshes on a dedicated `control-grants-changed` event.

use rusqlite::{params, OptionalExtension};

use super::{now_timestamp, Store, StoreError};

/// How long an approval lasts without being used: thirty days, renewed on
/// every authenticated reconnect.
pub const CONTROL_GRANT_LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// One `control_grants` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlGrantRecord {
    pub id: String,
    /// The name the client gave `stagedctl connect --client`, as approved.
    pub client: String,
    /// `None` = every project.
    pub scope_project_id: Option<String>,
    /// `"read"` or `"write"`.
    pub access: String,
    /// Hex SHA-256 of the reconnect secret handed to the client once.
    pub reconnect_secret_hash: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl ControlGrantRecord {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn is_expired_at(&self, now: i64) -> bool {
        self.expires_at <= now
    }

    pub fn writes(&self) -> bool {
        self.access == "write"
    }
}

const COLUMNS: &str = "id, client, scope_project_id, access, reconnect_secret_hash, \
                       created_at, expires_at, last_used_at, revoked_at";

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ControlGrantRecord> {
    Ok(ControlGrantRecord {
        id: row.get(0)?,
        client: row.get(1)?,
        scope_project_id: row.get(2)?,
        access: row.get(3)?,
        reconnect_secret_hash: row.get(4)?,
        created_at: row.get(5)?,
        expires_at: row.get(6)?,
        last_used_at: row.get(7)?,
        revoked_at: row.get(8)?,
    })
}

/// The renewed expiry for a grant used at `now`: exactly one lifetime out.
/// A reconnect can never push an expiry past that, however far out the row
/// already was.
pub fn renewed_expiry(now: i64) -> i64 {
    now + CONTROL_GRANT_LIFETIME_MS
}

impl Store {
    /// Insert an approved grant. `expires_at` is one lifetime from
    /// `created_at`.
    pub fn create_control_grant(&self, record: &ControlGrantRecord) -> Result<(), StoreError> {
        if record.access != "read" && record.access != "write" {
            return Err(StoreError(format!(
                "control grant access must be read or write, got {:?}",
                record.access
            )));
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            &format!(
                "INSERT INTO control_grants ({COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            ),
            params![
                record.id,
                record.client,
                record.scope_project_id,
                record.access,
                record.reconnect_secret_hash,
                record.created_at,
                record.expires_at,
                record.last_used_at,
                record.revoked_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_control_grant(&self, id: &str) -> Result<Option<ControlGrantRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM control_grants WHERE id = ?1"),
                params![id],
                row_to_record,
            )
            .optional()?)
    }

    /// Every grant that has not been revoked, newest first. Expired rows are
    /// included so the Settings panel can show them as expired; revoked
    /// rows are history and stay out of the list.
    pub fn list_control_grants(&self) -> Result<Vec<ControlGrantRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM control_grants WHERE revoked_at IS NULL \
             ORDER BY created_at DESC, id"
        ))?;
        let rows = stmt.query_map([], row_to_record)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// A successful reconnect at `now`: the expiry becomes
    /// [`renewed_expiry`]`(now)` and `last_used_at` is stamped. Returns the
    /// new expiry, or `None` if the row is missing or revoked (a revoked
    /// grant is never renewed back to life).
    pub fn renew_control_grant(&self, id: &str, now: i64) -> Result<Option<i64>, StoreError> {
        let expires_at = renewed_expiry(now);
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE control_grants SET expires_at = ?2, last_used_at = ?3 \
             WHERE id = ?1 AND revoked_at IS NULL",
            params![id, expires_at, now],
        )?;
        Ok((changed > 0).then_some(expires_at))
    }

    /// Record that a live bearer of this grant was used at `now`. Callers
    /// throttle; this is one UPDATE per call.
    pub fn touch_control_grant(&self, id: &str, now: i64) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE control_grants SET last_used_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now],
        )?;
        Ok(())
    }

    /// Stamp `revoked_at`. Returns whether a live (unrevoked) row was
    /// revoked; a second revoke is a no-op.
    pub fn revoke_control_grant(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE control_grants SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now_timestamp()],
        )?;
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Project;

    fn record(id: &str, project: Option<&str>, now: i64) -> ControlGrantRecord {
        ControlGrantRecord {
            id: id.to_string(),
            client: "my-agent".to_string(),
            scope_project_id: project.map(str::to_string),
            access: "write".to_string(),
            reconnect_secret_hash: format!("{id}-hash"),
            created_at: now,
            expires_at: renewed_expiry(now),
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn grants_round_trip_and_list_newest_first_without_revoked_rows() {
        let store = Store::in_memory().unwrap();
        let project = Project::named("Widgets");
        store.create_project(&project).unwrap();
        let older = record("g-old", Some(&project.id), 1_000);
        let newer = record("g-new", None, 2_000);
        store.create_control_grant(&older).unwrap();
        store.create_control_grant(&newer).unwrap();

        assert_eq!(store.get_control_grant("g-old").unwrap().unwrap(), older);
        assert_eq!(store.get_control_grant("missing").unwrap(), None);
        let listed: Vec<String> = store
            .list_control_grants()
            .unwrap()
            .into_iter()
            .map(|g| g.id)
            .collect();
        assert_eq!(listed, ["g-new", "g-old"]);

        assert!(store.revoke_control_grant("g-old").unwrap());
        assert!(!store.revoke_control_grant("g-old").unwrap(), "idempotent");
        assert!(!store.revoke_control_grant("missing").unwrap());
        let revoked = store.get_control_grant("g-old").unwrap().unwrap();
        assert!(revoked.is_revoked());
        let listed: Vec<String> = store
            .list_control_grants()
            .unwrap()
            .into_iter()
            .map(|g| g.id)
            .collect();
        assert_eq!(listed, ["g-new"], "revoked rows leave the list");
    }

    #[test]
    fn renewal_is_exactly_one_lifetime_from_now_and_never_more() {
        let store = Store::in_memory().unwrap();
        let mut far = record("g-far", None, 1_000);
        // A row somehow further out than a fresh renewal would put it.
        far.expires_at = 1_000 + 10 * CONTROL_GRANT_LIFETIME_MS;
        store.create_control_grant(&far).unwrap();
        let now = 5_000;
        assert_eq!(
            store.renew_control_grant("g-far", now).unwrap(),
            Some(now + CONTROL_GRANT_LIFETIME_MS)
        );
        let row = store.get_control_grant("g-far").unwrap().unwrap();
        assert_eq!(
            row.expires_at,
            now + CONTROL_GRANT_LIFETIME_MS,
            "renewal pulls an over-long expiry back to one lifetime"
        );
        assert_eq!(row.last_used_at, Some(now));

        // An expired row renews too — the caller decides whether an expired
        // grant may reconnect; the store just records the renewal asked of it.
        let later = now + 2 * CONTROL_GRANT_LIFETIME_MS;
        assert_eq!(
            store.renew_control_grant("g-far", later).unwrap(),
            Some(later + CONTROL_GRANT_LIFETIME_MS)
        );

        store.revoke_control_grant("g-far").unwrap();
        assert_eq!(
            store.renew_control_grant("g-far", later).unwrap(),
            None,
            "a revoked grant is never renewed back to life"
        );
        assert_eq!(store.renew_control_grant("missing", later).unwrap(), None);
    }

    #[test]
    fn touch_records_last_use_and_access_is_checked() {
        let store = Store::in_memory().unwrap();
        store.create_control_grant(&record("g", None, 1)).unwrap();
        store.touch_control_grant("g", 42).unwrap();
        assert_eq!(
            store.get_control_grant("g").unwrap().unwrap().last_used_at,
            Some(42)
        );
        let mut bad = record("bad", None, 1);
        bad.access = "admin".to_string();
        assert!(store.create_control_grant(&bad).is_err());
    }

    #[test]
    fn deleting_the_scoped_project_removes_its_grants() {
        let store = Store::in_memory().unwrap();
        let project = Project::named("Widgets");
        store.create_project(&project).unwrap();
        store
            .create_control_grant(&record("g", Some(&project.id), 1))
            .unwrap();
        store.delete_project(&project.id).unwrap();
        assert_eq!(
            store.get_control_grant("g").unwrap(),
            None,
            "scope_project_id cascades with the project"
        );
    }

    #[test]
    fn expiry_helpers_read_the_timestamps() {
        let row = record("g", None, 1_000);
        assert!(!row.is_expired_at(1_000));
        assert!(row.is_expired_at(renewed_expiry(1_000)));
        assert!(row.writes());
        assert!(!row.is_revoked());
    }
}
