use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

/// One rate-limit window of a plan, such as the 5-hour or weekly limit.
pub struct Window {
    pub label: String,
    /// Percent of the window used, 0–100.
    pub used: f64,
    /// When the window resets, as a Unix timestamp.
    pub resets: Option<i64>,
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn dir(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(default)
    })
}

fn get(url: &str, headers: &[(&str, &str)], cli: &str) -> Result<Value> {
    let mut req = ureq::get(url)
        .timeout(Duration::from_secs(10))
        .set("User-Agent", "herdr-usage");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.call() {
        Ok(resp) => resp.into_json().context("parsing usage"),
        Err(ureq::Error::Status(401 | 403, _)) => {
            bail!("login expired: start {cli} once to refresh it")
        }
        Err(ureq::Error::Status(code, _)) => bail!("usage request failed (HTTP {code})"),
        Err(e) => Err(anyhow!("usage request failed: {e}")),
    }
}

/// Claude Code's OAuth login, from its credentials file or the macOS keychain.
fn claude_credentials() -> Result<Value> {
    let file = dir("CLAUDE_CONFIG_DIR", ".claude").join(".credentials.json");
    let raw = match std::fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(_) if cfg!(target_os = "macos") => {
            let out = Command::new("security")
                .args([
                    "find-generic-password",
                    "-s",
                    "Claude Code-credentials",
                    "-w",
                ])
                .output()
                .context("reading the keychain")?;
            if !out.status.success() {
                bail!("Claude Code is not logged in");
            }
            String::from_utf8(out.stdout)?
        }
        Err(_) => bail!("Claude Code is not logged in"),
    };
    let v: Value = serde_json::from_str(&raw).context("parsing Claude credentials")?;
    Ok(v["claudeAiOauth"].clone())
}

pub fn claude() -> Result<Vec<Window>> {
    let creds = claude_credentials()?;
    if creds["expiresAt"]
        .as_i64()
        .is_some_and(|ms| ms / 1000 < now())
    {
        bail!("login expired: start claude once to refresh it");
    }
    let token = creds["accessToken"]
        .as_str()
        .context("Claude Code is not logged in with a subscription")?;
    let auth = format!("Bearer {token}");
    let v = get(
        "https://api.anthropic.com/api/oauth/usage",
        &[
            ("Authorization", &auth),
            ("anthropic-beta", "oauth-2025-04-20"),
        ],
        "claude",
    )?;
    let windows = [
        ("5h", "five_hour"),
        ("week", "seven_day"),
        ("opus", "seven_day_opus"),
        ("sonnet", "seven_day_sonnet"),
    ];
    Ok(windows
        .iter()
        .filter_map(|(label, key)| {
            let w = &v[key];
            Some(Window {
                label: (*label).into(),
                used: w["utilization"].as_f64()?,
                resets: w["resets_at"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| t.timestamp()),
            })
        })
        .collect())
}

fn span(secs: i64) -> String {
    match secs {
        s if s >= 6 * 86400 => "week".into(),
        s if s >= 86400 => format!("{}d", s / 86400),
        s => format!("{}h", (s + 1800) / 3600),
    }
}

pub fn codex() -> Result<Vec<Window>> {
    let file = dir("CODEX_HOME", ".codex").join("auth.json");
    let raw = std::fs::read_to_string(&file).context("Codex is not logged in")?;
    let v: Value = serde_json::from_str(&raw).context("parsing Codex auth")?;
    let tokens = &v["tokens"];
    let token = tokens["access_token"]
        .as_str()
        .context("Codex is not logged in with ChatGPT")?;
    let auth = format!("Bearer {token}");
    let mut headers = vec![("Authorization", auth.as_str())];
    if let Some(id) = tokens["account_id"].as_str() {
        headers.push(("chatgpt-account-id", id));
    }
    let u = get(
        "https://chatgpt.com/backend-api/wham/usage",
        &headers,
        "codex",
    )?;
    Ok(["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| {
            let w = &u["rate_limit"][key];
            Some(Window {
                label: span(w["limit_window_seconds"].as_i64()?),
                used: w["used_percent"].as_f64()?,
                resets: w["reset_at"].as_i64(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_name_common_windows() {
        assert_eq!(span(18000), "5h");
        assert_eq!(span(604800), "week");
        assert_eq!(span(2 * 86400), "2d");
    }
}
