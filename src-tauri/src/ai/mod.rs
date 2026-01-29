//! AI-powered diff analysis via ACP (Agent Client Protocol).
//!
//! Communicates with ACP-compatible agents like Goose to generate contextual
//! annotations for code changes.

mod acp_client;
mod prompt;
mod runner;
mod types;

pub use acp_client::{
    discover_acp_providers, find_acp_agent, find_acp_agent_by_id, run_acp_prompt,
    run_acp_prompt_with_session, AcpAgent, AcpPromptResult, AcpProviderInfo,
};
pub use runner::analyze_diff;
pub use types::{ChangesetAnalysis, ChangesetSummary, SmartDiffAnnotation, SmartDiffResult};
