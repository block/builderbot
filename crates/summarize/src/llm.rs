use anyhow::Result;
use serde_json::json;

use crate::auth::Auth;

/// Call the Databricks model endpoint with the given prompt.
///
/// Uses OpenAI-compatible chat completions API at:
///   {DATABRICKS_HOST}/serving-endpoints/{model}/invocations
///
/// Auth is resolved from environment (DATABRICKS_TOKEN or OAuth via DATABRICKS_HOST).
pub async fn complete(model: &str, prompt: &str) -> Result<String> {
    let auth = Auth::from_env()?;
    let host = resolve_host(&auth)?;
    let token = auth.get_token().await?;

    let url = format!(
        "{}/serving-endpoints/{}/invocations",
        host.trim_end_matches('/'),
        model
    );

    let system_message = "You are an assistant that analyzes content and provides clear, \
        concise summaries focused on answering the user's specific question. \
        Be specific and reference relevant parts of the content when helpful.";

    let payload = json!({
        "messages": [
            {"role": "system", "content": system_message},
            {"role": "user", "content": prompt}
        ]
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("LLM request failed ({}): {}", status, body);
    }

    let json: serde_json::Value = response.json().await?;

    // Extract the assistant message content from the OpenAI-compatible response
    let content = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Unexpected response format from LLM:\n{}",
                serde_json::to_string_pretty(&json).unwrap_or_default()
            )
        })?;

    Ok(content.to_string())
}

/// Resolve the Databricks host URL.
/// For token auth, DATABRICKS_HOST must be set separately.
/// For OAuth, the host is already embedded in the auth config.
fn resolve_host(auth: &Auth) -> Result<String> {
    match auth {
        Auth::Token(_) => std::env::var("DATABRICKS_HOST").map_err(|_| {
            anyhow::anyhow!("DATABRICKS_HOST must be set when using DATABRICKS_TOKEN auth")
        }),
        Auth::OAuth { host, .. } => Ok(host.clone()),
    }
}
