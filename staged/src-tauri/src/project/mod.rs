//! Project and Artifact types.
//!
//! Projects are goal-oriented collections of artifacts.
//! Artifacts are the persistent outputs of AI work (markdown documents, commits, etc.).
//!
//! Storage is handled by the unified Store (see `crate::store`).

// Re-export types from the unified store
pub use crate::store::{Artifact, ArtifactData, ArtifactStatus, ArtifactType, Project};
