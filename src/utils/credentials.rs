use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
struct OAuthCredentials {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<u64>,
    scopes: Option<Vec<String>>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthCredentials>,
}

pub fn get_oauth_token() -> Option<String> {
    if cfg!(target_os = "macos") {
        get_oauth_token_macos()
    } else {
        get_oauth_token_file()
    }
}

/// `CLAUDE_CONFIG_DIR`, when Claude Code runs with a configuration directory, and so an
/// account, other than the default one
fn custom_config_dir() -> Option<String> {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|dir| !dir.is_empty())
}

/// What Claude Code appends to its keychain entry for the account in use: nothing for
/// the default configuration directory, otherwise `-` and the first 8 hex digits of the
/// SHA-256 of `CLAUDE_CONFIG_DIR`. Claude Code hashes the path in Unicode NFC, which
/// ccline does not convert to.
pub fn account_suffix() -> String {
    suffix_for(custom_config_dir().as_deref())
}

fn suffix_for(config_dir: Option<&str>) -> String {
    let Some(dir) = config_dir else {
        return String::new();
    };
    let hash = ring::digest::digest(&ring::digest::SHA256, dir.as_bytes());
    let hex: String = hash.as_ref()[..4]
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect();
    format!("-{}", hex)
}

fn get_oauth_token_macos() -> Option<String> {
    use std::process::Command;

    // The keychain account Claude Code uses
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|user| {
            !user.is_empty()
                && user
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
        .unwrap_or_else(|| "claude-code-user".to_string());
    let service = format!("Claude Code-credentials{}", account_suffix());

    let output = Command::new("security")
        .args(["find-generic-password", "-a", &user, "-w", "-s", &service])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let json_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !json_str.is_empty() {
                if let Ok(creds_file) = serde_json::from_str::<CredentialsFile>(&json_str) {
                    return creds_file.claude_ai_oauth.map(|oauth| oauth.access_token);
                }
            }
            None
        }
        _ => {
            // Fallback to file-based credentials
            get_oauth_token_file()
        }
    }
}

/// The credentials file in Claude Code's configuration directory. Only the directory in
/// use is read, as another one holds another account's token.
fn get_oauth_token_file() -> Option<String> {
    let config_dir = match custom_config_dir() {
        Some(dir) => PathBuf::from(dir),
        None => dirs::home_dir()?.join(".claude"),
    };
    read_token_from_path(&config_dir.join(".credentials.json"))
}

/// Read OAuth token from a credentials file path
fn read_token_from_path(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(path).ok()?;
    let creds_file: CredentialsFile = serde_json::from_str(&content).ok()?;

    creds_file.claude_ai_oauth.map(|oauth| oauth.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_suffix_follows_claude_code() {
        assert_eq!(suffix_for(None), "");
        // sha256("/Users/x/.claude-work") starts with 74ed04d5
        assert_eq!(suffix_for(Some("/Users/x/.claude-work")), "-74ed04d5");
    }
}
