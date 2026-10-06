//! MCP server over stdio. Stdout carries the protocol; diagnostics go to stderr.
//!
//! Config (environment or `.env`): `MOODLE_URL`, `MOODLE_TOKEN` (raw token or
//! `moodledl://token=...` URL), optional `MOODLE_MCP_DIR` (default `~/.moodle-mcp`)
//! and `MOODLE_MCP_MAX_FILE_MB` (default 100).

mod cache;
mod server;

use api::Client;
use api::auth::SsoToken;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let base = std::env::var("MOODLE_URL").map_err(|_| "set MOODLE_URL")?;
    let token = std::env::var("MOODLE_TOKEN").map_err(|_| "set MOODLE_TOKEN")?;
    let token = if token.contains("token=") {
        SsoToken::parse(&token)
            .ok_or("could not decode MOODLE_TOKEN")?
            .token
    } else {
        token
    };

    let mut options = sync::Options::default();
    if let Ok(mb) = std::env::var("MOODLE_MCP_MAX_FILE_MB") {
        let mb: u64 = mb
            .parse()
            .map_err(|_| "MOODLE_MCP_MAX_FILE_MB must be a number")?;
        options.max_file_size = Some(mb * 1024 * 1024);
    }

    let client = Client::new(&base, token)?;
    let cache = cache::Cache::from_env()?;

    let service = server::MoodleMcp::new(client, cache, options)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
