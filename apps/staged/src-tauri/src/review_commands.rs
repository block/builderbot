//! Review commands — code review CRUD operations.

use crate::git;
use crate::store::Store;
use std::sync::{Arc, Mutex};

/// Get or create a review for a branch + commit + scope.
///
/// This is the "lazy create" entry point — called when the user does
/// their first persistent action (comment, mark reviewed, etc.).
/// If a review already exists for this triple, returns it.
#[tauri::command(rename_all = "camelCase")]
pub async fn ensure_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
) -> Result<crate::store::Review, String> {
    let store = crate::get_store(&store)?;
    let review_scope = crate::store::ReviewScope::parse(&scope)
        .ok_or_else(|| format!("Invalid scope: {scope}"))?;

    store
        .ensure_review(&branch_id, &commit_sha, review_scope)
        .map_err(|e| e.to_string())
}

/// Find an existing review by (branch, commit, scope) without creating one.
#[tauri::command(rename_all = "camelCase")]
pub async fn find_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
) -> Result<Option<crate::store::Review>, String> {
    let store = crate::get_store(&store)?;
    let review_scope = crate::store::ReviewScope::parse(&scope)
        .ok_or_else(|| format!("Invalid scope: {scope}"))?;

    store
        .find_review(&branch_id, &commit_sha, review_scope)
        .map_err(|e| e.to_string())
}

/// Get a review by ID with all child data.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
) -> Result<Option<crate::store::Review>, String> {
    crate::get_store(&store)?
        .get_review(&review_id)
        .map_err(|e| e.to_string())
}

/// Mark a file as reviewed.
#[tauri::command(rename_all = "camelCase")]
pub async fn mark_reviewed(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    crate::get_store(&store)?
        .mark_reviewed(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Unmark a file as reviewed.
#[tauri::command(rename_all = "camelCase")]
pub async fn unmark_reviewed(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    crate::get_store(&store)?
        .unmark_reviewed(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Add a comment to a review.
#[tauri::command(rename_all = "camelCase")]
pub async fn add_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
    span_start: u32,
    span_end: u32,
    content: String,
) -> Result<crate::store::Comment, String> {
    let store = crate::get_store(&store)?;
    let comment = crate::store::Comment::new(&path, git::Span::new(span_start, span_end), &content);
    store
        .add_comment(&review_id, &comment)
        .map_err(|e| e.to_string())?;
    Ok(comment)
}

/// Update a comment's content.
#[tauri::command(rename_all = "camelCase")]
pub async fn update_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    comment_id: String,
    content: String,
) -> Result<(), String> {
    crate::get_store(&store)?
        .update_comment(&comment_id, &content)
        .map_err(|e| e.to_string())
}

/// Delete a comment.
#[tauri::command(rename_all = "camelCase")]
pub async fn delete_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    comment_id: String,
) -> Result<(), String> {
    crate::get_store(&store)?
        .delete_comment(&comment_id)
        .map_err(|e| e.to_string())
}

/// Add a reference file to a review.
#[tauri::command(rename_all = "camelCase")]
pub async fn add_reference_file(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    crate::get_store(&store)?
        .add_reference_file(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Remove a reference file from a review.
#[tauri::command(rename_all = "camelCase")]
pub async fn remove_reference_file(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    crate::get_store(&store)?
        .remove_reference_file(&review_id, &path)
        .map_err(|e| e.to_string())
}
