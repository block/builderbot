//! Repo badge CRUD and hue-assignment algorithm.

use rusqlite::params;

use super::models::RepoBadge;
use super::{Store, StoreError};

/// All columns selected in badge queries, in a fixed order.
const BADGE_COLUMNS: &str =
    "github_repo, subpath, short_name, hue, created_at, pinned, pin_sort_order, default_branch";

fn row_to_badge(row: &rusqlite::Row) -> rusqlite::Result<RepoBadge> {
    let pinned_int: i32 = row.get(5)?;
    Ok(RepoBadge {
        github_repo: row.get(0)?,
        subpath: row.get(1)?,
        short_name: row.get(2)?,
        hue: row.get(3)?,
        created_at: row.get(4)?,
        pinned: pinned_int != 0,
        pin_sort_order: row.get(6)?,
        default_branch: row.get(7)?,
    })
}

impl Store {
    /// Fetch all repo badges.
    pub fn list_repo_badges(&self) -> Result<Vec<RepoBadge>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {BADGE_COLUMNS} FROM repo_badges ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map([], row_to_badge)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Look up a single badge by (github_repo, subpath).
    pub fn get_repo_badge(
        &self,
        github_repo: &str,
        subpath: &str,
    ) -> Result<Option<RepoBadge>, StoreError> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {BADGE_COLUMNS} FROM repo_badges
                 WHERE github_repo = ?1 AND subpath = ?2"
            ),
            params![github_repo, subpath],
            row_to_badge,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Insert a new badge. Fails on duplicate (github_repo, subpath) or short_name.
    pub fn create_repo_badge(&self, badge: &RepoBadge) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO repo_badges (github_repo, subpath, short_name, hue, created_at, pinned, pin_sort_order, default_branch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                badge.github_repo,
                badge.subpath,
                badge.short_name,
                badge.hue,
                badge.created_at,
                badge.pinned as i32,
                badge.pin_sort_order,
                badge.default_branch,
            ],
        )?;
        Ok(())
    }

    /// Update the short_name and hue of an existing badge.
    /// Returns an error if no badge exists for the given repo+subpath.
    pub fn update_repo_badge(
        &self,
        github_repo: &str,
        subpath: &str,
        short_name: &str,
        hue: f64,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE repo_badges SET short_name = ?1, hue = ?2
             WHERE github_repo = ?3 AND subpath = ?4",
            params![short_name, hue, github_repo, subpath],
        )?;
        if rows == 0 {
            return Err(StoreError(format!(
                "No badge found for {github_repo} subpath={subpath}"
            )));
        }
        Ok(())
    }

    /// Look up a single badge by its short_name.
    pub fn get_repo_badge_by_short_name(
        &self,
        short_name: &str,
    ) -> Result<Option<RepoBadge>, StoreError> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {BADGE_COLUMNS} FROM repo_badges WHERE short_name = ?1"),
            params![short_name],
            row_to_badge,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Delete a badge by (github_repo, subpath).
    pub fn delete_repo_badge(&self, github_repo: &str, subpath: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM repo_badges WHERE github_repo = ?1 AND subpath = ?2",
            params![github_repo, subpath],
        )?;
        Ok(())
    }

    /// Return all existing hue values (sorted ascending).
    pub fn list_badge_hues(&self) -> Result<Vec<f64>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT hue FROM repo_badges ORDER BY hue ASC")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // =========================================================================
    // Pinning operations
    // =========================================================================

    /// Pin a repo badge — sets `pinned=1` and assigns the next `pin_sort_order`.
    pub fn pin_repo(&self, github_repo: &str, subpath: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let max_order: Option<i32> = conn.query_row(
            "SELECT MAX(pin_sort_order) FROM repo_badges WHERE pinned = 1",
            [],
            |row| row.get(0),
        )?;
        let next_order = max_order.map_or(0, |m| m + 1);
        let rows = conn.execute(
            "UPDATE repo_badges SET pinned = 1, pin_sort_order = ?1
             WHERE github_repo = ?2 AND subpath = ?3",
            params![next_order, github_repo, subpath],
        )?;
        if rows == 0 {
            return Err(StoreError(format!(
                "No badge found for {github_repo} subpath={subpath}"
            )));
        }
        Ok(())
    }

    /// Unpin a repo badge — sets `pinned=0` and clears `pin_sort_order`.
    pub fn unpin_repo(&self, github_repo: &str, subpath: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE repo_badges SET pinned = 0, pin_sort_order = NULL
             WHERE github_repo = ?1 AND subpath = ?2",
            params![github_repo, subpath],
        )?;
        if rows == 0 {
            return Err(StoreError(format!(
                "No badge found for {github_repo} subpath={subpath}"
            )));
        }
        Ok(())
    }

    /// Bulk-update `pin_sort_order` from the given ordered list of (github_repo, subpath) keys.
    pub fn reorder_pinned_repos(
        &self,
        ordered_keys: &[(String, String)],
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        for (i, (github_repo, subpath)) in ordered_keys.iter().enumerate() {
            conn.execute(
                "UPDATE repo_badges SET pin_sort_order = ?1
                 WHERE github_repo = ?2 AND subpath = ?3 AND pinned = 1",
                params![i as i32, github_repo, subpath],
            )?;
        }
        Ok(())
    }

    /// List all repo badges ordered: pinned first by `pin_sort_order`, then unpinned
    /// sorted by project count descending (number of projects using that repo+subpath).
    pub fn list_repos_for_home(&self) -> Result<Vec<RepoBadge>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {BADGE_COLUMNS}
             FROM repo_badges rb
             LEFT JOIN (
                 SELECT github_repo, subpath, COUNT(*) AS project_count
                 FROM project_repos
                 GROUP BY github_repo, subpath
             ) pc ON rb.github_repo = pc.github_repo AND rb.subpath = pc.subpath
             ORDER BY rb.pinned DESC, rb.pin_sort_order ASC, COALESCE(pc.project_count, 0) DESC"
        ))?;
        let rows = stmt.query_map([], row_to_badge)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Store the detected default branch for a repo badge.
    pub fn set_default_branch(
        &self,
        github_repo: &str,
        subpath: &str,
        default_branch: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE repo_badges SET default_branch = ?1
             WHERE github_repo = ?2 AND subpath = ?3",
            params![default_branch, github_repo, subpath],
        )?;
        if rows == 0 {
            return Err(StoreError(format!(
                "No badge found for {github_repo} subpath={subpath}"
            )));
        }
        Ok(())
    }
}

/// Choose the next hue that maximises visual distance from all existing hues.
///
/// Treats the hue wheel as a circular [0, 360) range. Finds the largest gap
/// between consecutive hues and places the new one at the midpoint.
/// If no hues exist yet, returns 210 (blue).
pub fn next_hue(existing: &[f64]) -> f64 {
    if existing.is_empty() {
        return 210.0;
    }
    let mut sorted = existing.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut best_gap = 0.0_f64;
    let mut best_mid = 210.0_f64;
    for i in 0..sorted.len() {
        let next = if i + 1 < sorted.len() {
            sorted[i + 1]
        } else {
            sorted[0] + 360.0
        };
        let gap = next - sorted[i];
        if gap > best_gap {
            best_gap = gap;
            best_mid = (sorted[i] + gap / 2.0) % 360.0;
        }
    }
    best_mid
}

/// Generate a deterministic fallback short name from a repo+subpath.
///
/// Takes the last meaningful path segment, lowercases it, strips non-alphanumeric
/// chars, and truncates to 6 chars. Appends digits if needed for uniqueness.
pub fn fallback_short_name(github_repo: &str, subpath: &str, taken: &[String]) -> String {
    let segment = if !subpath.is_empty() {
        subpath.rsplit('/').next().unwrap_or(subpath)
    } else {
        github_repo.rsplit('/').next().unwrap_or(github_repo)
    };
    let base: String = segment
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(6)
        .collect();
    let base = if base.is_empty() {
        "repo".to_string()
    } else {
        base
    };
    if !taken.contains(&base) {
        return base;
    }
    // Append digits to find a unique name
    for i in 2..=99 {
        let suffix = i.to_string();
        let max_base = 6 - suffix.len();
        let candidate = format!("{}{}", &base[..base.len().min(max_base)], suffix);
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    // Extremely unlikely fallback — mix timestamp with random seed for uniqueness
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let random_bits = RandomState::new().build_hasher().finish() as u32;
    format!("{:05x}", random_bits % 0xFFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_hue_empty() {
        assert_eq!(next_hue(&[]), 210.0);
    }

    #[test]
    fn test_next_hue_single() {
        // Single hue at 210 -> largest gap is 360 degrees, midpoint at 210+180=30
        let result = next_hue(&[210.0]);
        assert!((result - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_next_hue_two() {
        // Hues at 0 and 180 -> two equal 180 degree gaps, picks midpoint of first = 90
        let result = next_hue(&[0.0, 180.0]);
        assert!((result - 90.0).abs() < 0.01);
    }

    #[test]
    fn test_fallback_short_name_basic() {
        assert_eq!(fallback_short_name("block/goose", "", &[]), "goose");
        assert_eq!(fallback_short_name("block/builderbot", "", &[]), "builde");
        assert_eq!(
            fallback_short_name("block/wallet", "apps/server", &[]),
            "server"
        );
    }

    #[test]
    fn test_fallback_short_name_dedup() {
        let taken = vec!["goose".to_string()];
        let result = fallback_short_name("block/goose", "", &taken);
        assert_eq!(result, "goose2");
    }
}
