//! Repo affinity tracking — records which repos are used together in projects.

use rusqlite::params;

use super::models::SuggestedRepo;
use super::{now_timestamp, Store, StoreError};

/// Build the canonical affinity key for a repo, encoding the subpath when present.
/// The `::` delimiter is safe because neither GitHub `owner/repo` names nor
/// filesystem subpaths can contain `::`, so there is no collision risk.
pub fn repo_affinity_key(github_repo: &str, subpath: Option<&str>) -> String {
    match subpath.filter(|s| !s.is_empty()) {
        Some(sub) => format!("{github_repo}::{sub}"),
        None => github_repo.to_string(),
    }
}

/// Parse a repo affinity key back into (github_repo, subpath).
fn parse_affinity_key(key: &str) -> (String, Option<String>) {
    match key.split_once("::") {
        Some((repo, sub)) => (repo.to_string(), Some(sub.to_string())),
        None => (key.to_string(), None),
    }
}

impl Store {
    /// Record (or increment) an affinity between two repo keys.
    ///
    /// Keys are stored in lexicographic order so `(A,B)` and `(B,A)` map to
    /// the same row.
    pub fn record_repo_affinity(&self, key_a: &str, key_b: &str) -> Result<(), StoreError> {
        // Skip self-affinity (e.g. same repo+subpath added twice).
        if key_a == key_b {
            return Ok(());
        }
        let (a, b) = if key_a <= key_b {
            (key_a, key_b)
        } else {
            (key_b, key_a)
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO repo_affinities (repo_a, repo_b, co_use_count, last_seen_at)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(repo_a, repo_b) DO UPDATE
                SET co_use_count = co_use_count + 1,
                    last_seen_at = ?3",
            params![a, b, now_timestamp()],
        )?;
        Ok(())
    }

    /// Query repos that have historically been paired with the given keys,
    /// excluding those already in `current_keys`. Results are sorted by
    /// aggregate affinity score (descending).
    pub fn get_suggested_repos(
        &self,
        current_keys: &[String],
        limit: usize,
    ) -> Result<Vec<SuggestedRepo>, StoreError> {
        if current_keys.is_empty() {
            return self.get_popular_repos(limit);
        }

        let conn = self.conn.lock().unwrap();

        // Build placeholders for the IN clauses.
        let placeholders: String = current_keys
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        // Offset for the second copy of params (exclude list) and the third copy.
        let n = current_keys.len();
        let placeholders2: String = (0..n)
            .map(|i| format!("?{}", n + i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let exclude_placeholders: String = (0..n)
            .map(|i| format!("?{}", 2 * n + i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT repo_key, SUM(co_use_count) as score
             FROM (
                 SELECT repo_b as repo_key, co_use_count FROM repo_affinities WHERE repo_a IN ({placeholders})
                 UNION ALL
                 SELECT repo_a as repo_key, co_use_count FROM repo_affinities WHERE repo_b IN ({placeholders2})
             )
             WHERE repo_key NOT IN ({exclude_placeholders})
             GROUP BY repo_key
             ORDER BY score DESC
             LIMIT ?{}",
            3 * n + 1
        );

        let mut stmt = conn.prepare(&sql)?;

        // Bind parameters: current_keys x3 (two IN clauses + exclude), then limit.
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for _ in 0..3 {
            for key in current_keys {
                param_values.push(Box::new(key.clone()));
            }
        }
        param_values.push(Box::new(limit as i64));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            let key: String = row.get(0)?;
            let score: i64 = row.get(1)?;
            Ok((key, score))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (key, score) = row?;
            let (github_repo, subpath) = parse_affinity_key(&key);
            results.push(SuggestedRepo {
                github_repo,
                subpath,
                score,
            });
        }
        Ok(results)
    }

    /// Return the most-added repos across all projects, ranked by distinct
    /// project count. Used as a fallback when the current project has no repos.
    fn get_popular_repos(&self, limit: usize) -> Result<Vec<SuggestedRepo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT github_repo, subpath, COUNT(DISTINCT project_id) as score
             FROM project_repos
             GROUP BY github_repo, COALESCE(subpath, '')
             ORDER BY score DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let github_repo: String = row.get(0)?;
            let subpath: Option<String> = row.get(1)?;
            let score: i64 = row.get(2)?;
            Ok(SuggestedRepo {
                github_repo,
                subpath,
                score,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}
