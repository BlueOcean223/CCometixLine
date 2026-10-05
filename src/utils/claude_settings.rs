//! Values from Claude Code's settings files.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The Claude Code settings files that apply to a project, highest priority first:
/// the project's `.claude/settings.local.json` and `.claude/settings.json`, then the
/// user's `settings.json` in `CLAUDE_CONFIG_DIR` (`~/.claude` by default). Managed
/// settings and files passed with `--settings` are not read.
pub struct ClaudeSettings {
    files: Vec<Value>,
}

impl ClaudeSettings {
    /// Read the settings of the project in `project_dir`, or only the user's
    /// settings without a project
    pub fn load(project_dir: Option<&Path>) -> Self {
        Self::read(&settings_paths(project_dir, config_dir().as_deref()))
    }

    /// Missing or malformed files are skipped
    fn read(paths: &[PathBuf]) -> Self {
        let files = paths
            .iter()
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .filter_map(|content| serde_json::from_str(&content).ok())
            .collect();
        Self { files }
    }

    /// The value at `path` in the highest-priority file that sets it. Claude Code
    /// merges the files key by key, so this is the value it uses, except for arrays,
    /// which it concatenates.
    pub fn get(&self, path: &[&str]) -> Option<&Value> {
        self.files
            .iter()
            .find_map(|settings| path.iter().try_fold(settings, |value, key| value.get(key)))
    }
}

/// Claude Code's configuration directory: `CLAUDE_CONFIG_DIR`, or `~/.claude`
fn config_dir() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".claude")),
    }
}

fn settings_paths(project_dir: Option<&Path>, config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(project) = project_dir {
        paths.push(project.join(".claude").join("settings.local.json"));
        paths.push(project.join(".claude").join("settings.json"));
    }
    paths.extend(config_dir.map(|dir| dir.join("settings.json")));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn higher_priority_files_win_key_by_key() {
        let dir = std::env::temp_dir().join(format!("ccline-settings-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let project = dir.join("project");
        let config = dir.join("config");
        fs::create_dir_all(project.join(".claude")).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(
            project.join(".claude").join("settings.local.json"),
            r#"{"statusLine":{"padding":2}}"#,
        )
        .unwrap();
        fs::write(
            project.join(".claude").join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"ccline","padding":1}}"#,
        )
        .unwrap();
        fs::write(
            config.join("settings.json"),
            r#"{"statusLine":{"padding":0},"env":{"HTTPS_PROXY":"http://user"}}"#,
        )
        .unwrap();

        let settings = ClaudeSettings::read(&settings_paths(Some(&project), Some(&config)));
        assert_eq!(
            settings.get(&["statusLine", "padding"]),
            Some(&Value::from(2))
        );
        assert_eq!(
            settings.get(&["statusLine", "command"]),
            Some(&Value::from("ccline"))
        );
        assert_eq!(
            settings.get(&["env", "HTTPS_PROXY"]),
            Some(&Value::from("http://user"))
        );
        assert_eq!(settings.get(&["env", "HTTP_PROXY"]), None);

        // Without a project only the user's settings apply; a malformed file is skipped
        fs::write(config.join("settings.json"), "{").unwrap();
        let settings = ClaudeSettings::read(&settings_paths(None, Some(&config)));
        assert_eq!(settings.get(&["statusLine", "padding"]), None);

        let _ = fs::remove_dir_all(&dir);
    }
}
