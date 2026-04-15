//! Repo badge CRUD and hue-assignment algorithm.

use rusqlite::params;

use super::models::RepoBadge;
use super::{Store, StoreError};

impl Store {
    /// Fetch all repo badges.
    pub fn list_repo_badges(&self) -> Result<Vec<RepoBadge>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT github_repo, subpath, short_name, hue, created_at
             FROM repo_badges
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RepoBadge {
                github_repo: row.get(0)?,
                subpath: row.get(1)?,
                short_name: row.get(2)?,
                hue: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
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
            "SELECT github_repo, subpath, short_name, hue, created_at
             FROM repo_badges
             WHERE github_repo = ?1 AND subpath = ?2",
            params![github_repo, subpath],
            |row| {
                Ok(RepoBadge {
                    github_repo: row.get(0)?,
                    subpath: row.get(1)?,
                    short_name: row.get(2)?,
                    hue: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// Insert a new badge. Fails on duplicate (github_repo, subpath) or short_name.
    pub fn create_repo_badge(&self, badge: &RepoBadge) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO repo_badges (github_repo, subpath, short_name, hue, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                badge.github_repo,
                badge.subpath,
                badge.short_name,
                badge.hue,
                badge.created_at,
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
            "SELECT github_repo, subpath, short_name, hue, created_at
             FROM repo_badges
             WHERE short_name = ?1",
            params![short_name],
            |row| {
                Ok(RepoBadge {
                    github_repo: row.get(0)?,
                    subpath: row.get(1)?,
                    short_name: row.get(2)?,
                    hue: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
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
