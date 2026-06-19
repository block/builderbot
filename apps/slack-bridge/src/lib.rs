//! Minimal Slack ↔ builderbot bridge.
//!
//! Pure, side-effect-free building blocks live here so they can be unit-tested
//! in isolation:
//! - [`events`] parses Slack `app_mention` payloads into tasks.
//! - [`progress`] accumulates streamed agent output into a Slack message body.
//!
//! The binary (`main.rs`) wires these into a live Socket Mode connection.

pub mod config;
pub mod events;
pub mod progress;
pub mod runner;
pub mod slack;
