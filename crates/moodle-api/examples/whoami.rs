//! Logs in via SSO (or an existing token) and prints who you are.
//!
//! ```sh
//! cp example.env .env   # then set MOODLE_URL
//! cargo run -p moodle-api --example whoami
//! ```
//!
//! Variables are read from the environment or a `.env` file (current directory
//! or any parent). Set `MOODLE_TOKEN` to skip the browser login. It accepts a
//! raw `wstoken` or the full `moodledl://token=...` redirect URL.

use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead, Write};

use moodle_api::Client;
use moodle_api::auth::{LaunchRequest, SsoToken};
use moodle_api::site::GetSiteInfo;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A missing `.env` is fine; real environment variables take precedence.
    dotenvy::dotenv().ok();

    let base = std::env::var("MOODLE_URL")
        .map_err(|_| "set MOODLE_URL, e.g. https://moodle.example.edu/")?;
    let base = if base.ends_with('/') {
        base
    } else {
        format!("{base}/")
    };

    let token = match std::env::var("MOODLE_TOKEN") {
        Ok(t) if t.contains("token=") => parse(&t)?,
        Ok(t) if !t.is_empty() => t,
        _ => sso_login(&base)?,
    };

    let client = Client::new(&base, token)?;
    let info = client.call(&GetSiteInfo).await?;

    println!("Site:     {} ({})", info.sitename, info.siteurl);
    println!(
        "User:     {} ({}, id {})",
        info.fullname, info.username, info.userid
    );
    println!("Release:  {}", info.release.as_deref().unwrap_or("unknown"));
    println!(
        "Functions available to this token: {}",
        info.functions.len()
    );
    Ok(())
}

fn sso_login(base: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Random enough for a nonce, without pulling in `rand`.
    let passport = format!("{:016x}", RandomState::new().hash_one(0u8));
    let launch = LaunchRequest {
        service: "moodle_mobile_app".into(),
        passport,
        urlscheme: "moodledl".into(),
    };

    println!(
        "1. Open this URL in a browser and log in:\n\n   {}\n",
        launch.url(base)
    );
    println!("2. The browser will fail to open a `moodledl://token=...` link.");
    println!("   Copy it (e.g. from the dev tools console or network tab).\n");
    print!("Paste the moodledl:// URL: ");
    io::stdout().flush()?;

    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    parse(line.trim())
}

fn parse(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    let sso = SsoToken::parse(input).ok_or("could not decode the token from that URL")?;
    Ok(sso.token)
}
