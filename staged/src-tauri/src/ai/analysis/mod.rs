//! Legacy diff analysis module - REFERENCE CODE ONLY.
//!
//! This module contains the original AI-powered diff analysis implementation.
//! It is NOT wired up to any commands - kept purely as a code reference for
//! prompt construction patterns and diff analysis approaches.
//!
//! If you want to revive this functionality, you'll need to:
//! 1. Add Tauri commands in lib.rs
//! 2. Add storage functions in review/mod.rs
//! 3. Wire up frontend calls

#![allow(dead_code)]
#![allow(unused_imports)]

mod prompt;
pub mod runner;
pub mod types;

pub use runner::analyze_diff;
pub use types::ChangesetAnalysis;
