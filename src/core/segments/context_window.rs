use super::{Segment, SegmentData};
use crate::config::{InputData, ModelConfig, SegmentId, TranscriptEntry};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct ContextWindowSegment;

impl ContextWindowSegment {
    pub fn new() -> Self {
        Self
    }
}

/// Resolve `(tokens used, context limit)` for the current model.
///
/// The limit is the smaller of the model's real window (a models.toml entry) and the
/// window Claude Code works with: a larger real window is cut short by Claude Code's
/// auto-compact, a smaller one by API errors. Claude Code reports its window as
/// `context_window_size`; older versions infer it from the model ID.
///
/// Tokens come from `context_window.current_usage`. While it is null (before the
/// session's first API response and right after `/compact`), only the session's own
/// transcript can stand in, which has usage when the session was resumed. Usage from
/// before a compaction does not count, so `/compact` shows no usage until the next
/// response. Older Claude Code versions without `context_window` use the transcript
/// fallback, which may also take usage from the project's latest session.
fn resolve_context(input: &InputData, model_config: &ModelConfig) -> (Option<u32>, u32) {
    let native = input.context_window.as_ref();

    let claude_code_limit = native
        .and_then(|cw| cw.context_window_size)
        .filter(|&size| size > 0)
        .unwrap_or_else(|| model_config.get_inferred_context_limit(&input.model.id));
    let context_limit = match model_config.get_entry_context_limit(&input.model.id) {
        Some(model_limit) => model_limit.min(claude_code_limit),
        None => claude_code_limit,
    };

    let tokens = match native {
        Some(cw) => cw
            .current_usage
            .as_ref()
            .map(|usage| usage.context_tokens())
            .or_else(|| try_parse_transcript_file(Path::new(&input.transcript_path))),
        None => parse_transcript_usage(&input.transcript_path),
    };

    (tokens, context_limit)
}

impl Segment for ContextWindowSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let (context_used_token_opt, context_limit) = resolve_context(input, &ModelConfig::load());

        let (percentage_display, tokens_display) = match context_used_token_opt {
            Some(context_used_token) => {
                let context_used_rate = (context_used_token as f64 / context_limit as f64) * 100.0;

                let percentage = if context_used_rate.fract() == 0.0 {
                    format!("{:.0}%", context_used_rate)
                } else {
                    format!("{:.1}%", context_used_rate)
                };

                let tokens = if context_used_token >= 1000 {
                    let k_value = context_used_token as f64 / 1000.0;
                    if k_value.fract() == 0.0 {
                        format!("{}k", k_value as u32)
                    } else {
                        format!("{:.1}k", k_value)
                    }
                } else {
                    context_used_token.to_string()
                };

                (percentage, tokens)
            }
            None => {
                // No usage data available
                ("-".to_string(), "-".to_string())
            }
        };

        let mut metadata = HashMap::new();
        match context_used_token_opt {
            Some(context_used_token) => {
                let context_used_rate = (context_used_token as f64 / context_limit as f64) * 100.0;
                metadata.insert("tokens".to_string(), context_used_token.to_string());
                metadata.insert("percentage".to_string(), context_used_rate.to_string());
            }
            None => {
                metadata.insert("tokens".to_string(), "-".to_string());
                metadata.insert("percentage".to_string(), "-".to_string());
            }
        }
        metadata.insert("limit".to_string(), context_limit.to_string());
        metadata.insert("model".to_string(), input.model.id.clone());

        Some(SegmentData {
            primary: format!("{} · {} tokens", percentage_display, tokens_display),
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::ContextWindow
    }
}

fn parse_transcript_usage<P: AsRef<Path>>(transcript_path: P) -> Option<u32> {
    let path = transcript_path.as_ref();

    // Try to parse from current transcript file
    if let Some(usage) = try_parse_transcript_file(path) {
        return Some(usage);
    }

    // If file doesn't exist, try to find usage from project history
    if !path.exists() {
        if let Some(usage) = try_find_usage_from_project_history(path) {
            return Some(usage);
        }
    }

    None
}

fn try_parse_transcript_file(path: &Path) -> Option<u32> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    if lines.is_empty() {
        return None;
    }

    // Check if the last line is a summary
    let last_line = lines.last()?.trim();
    if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(last_line) {
        if entry.r#type.as_deref() == Some("summary") {
            // Handle summary case: find usage by leafUuid
            if let Some(leaf_uuid) = &entry.leaf_uuid {
                let project_dir = path.parent()?;
                return find_usage_by_leaf_uuid(leaf_uuid, project_dir);
            }
        }
    }

    // Normal case: find the last assistant message in current file. Usage from before a
    // compaction boundary describes the context that /compact replaced.
    for line in lines.iter().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            if entry.r#type.as_deref() == Some("system")
                && entry.subtype.as_deref() == Some("compact_boundary")
            {
                return None;
            }
            if entry.r#type.as_deref() == Some("assistant") {
                if let Some(message) = &entry.message {
                    if let Some(raw_usage) = &message.usage {
                        let normalized = raw_usage.clone().normalize();
                        return Some(normalized.display_tokens());
                    }
                }
            }
        }
    }

    None
}

fn find_usage_by_leaf_uuid(leaf_uuid: &str, project_dir: &Path) -> Option<u32> {
    // Search for the leafUuid across all session files in the project directory
    let entries = fs::read_dir(project_dir).ok()?;

    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        if let Some(usage) = search_uuid_in_file(&path, leaf_uuid) {
            return Some(usage);
        }
    }

    None
}

fn search_uuid_in_file(path: &Path, target_uuid: &str) -> Option<u32> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    // Find the message with target_uuid
    for line in &lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            if let Some(uuid) = &entry.uuid {
                if uuid == target_uuid {
                    // Found the target message, check its type
                    if entry.r#type.as_deref() == Some("assistant") {
                        // Direct assistant message with usage
                        if let Some(message) = &entry.message {
                            if let Some(raw_usage) = &message.usage {
                                let normalized = raw_usage.clone().normalize();
                                return Some(normalized.display_tokens());
                            }
                        }
                    } else if entry.r#type.as_deref() == Some("user") {
                        // User message, need to find the parent assistant message
                        if let Some(parent_uuid) = &entry.parent_uuid {
                            return find_assistant_message_by_uuid(&lines, parent_uuid);
                        }
                    }
                    break;
                }
            }
        }
    }

    None
}

fn find_assistant_message_by_uuid(lines: &[String], target_uuid: &str) -> Option<u32> {
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            if let Some(uuid) = &entry.uuid {
                if uuid == target_uuid && entry.r#type.as_deref() == Some("assistant") {
                    if let Some(message) = &entry.message {
                        if let Some(raw_usage) = &message.usage {
                            let normalized = raw_usage.clone().normalize();
                            return Some(normalized.display_tokens());
                        }
                    }
                }
            }
        }
    }

    None
}

fn try_find_usage_from_project_history(transcript_path: &Path) -> Option<u32> {
    let project_dir = transcript_path.parent()?;

    // Find the most recent session file in the project directory
    let mut session_files: Vec<PathBuf> = Vec::new();
    let entries = fs::read_dir(project_dir).ok()?;

    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            session_files.push(path);
        }
    }

    if session_files.is_empty() {
        return None;
    }

    // Sort by modification time (most recent first)
    session_files.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    session_files.reverse();

    // Try to find usage from the most recent session
    for session_path in &session_files {
        if let Some(usage) = try_parse_transcript_file(session_path) {
            return Some(usage);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(model_id: &str, extra: &str) -> InputData {
        let json = format!(
            r#"{{"model":{{"id":"{}","display_name":"Model"}},"workspace":{{"current_dir":"/tmp"}},"transcript_path":"/nonexistent/transcript.jsonl"{}}}"#,
            model_id, extra
        );
        serde_json::from_str(&json).unwrap()
    }

    const USAGE: &str = r#""current_usage":{"input_tokens":5000,"output_tokens":1200,"cache_creation_input_tokens":10000,"cache_read_input_tokens":80000}"#;

    #[test]
    fn uses_native_usage_and_size() {
        let input = input(
            "claude-opus-5-5",
            &format!(
                r#","context_window":{{"context_window_size":1000000,{}}}"#,
                USAGE
            ),
        );
        // Includes the last response's output tokens, like the transcript fallback
        assert_eq!(
            resolve_context(&input, &ModelConfig::default()),
            (Some(96_200), 1_000_000)
        );
    }

    #[test]
    fn smaller_real_window_wins() {
        // Declared as 1M to Claude Code, but the model only serves 262k
        let input = input(
            "kimi-k2.7-code[1m]",
            &format!(
                r#","context_window":{{"context_window_size":1000000,{}}}"#,
                USAGE
            ),
        );
        assert_eq!(resolve_context(&input, &ModelConfig::default()).1, 262_144);
    }

    #[test]
    fn smaller_claude_code_window_wins() {
        // A 1M model without the [1m] declaration is compacted at Claude Code's 200k
        let input = input(
            "deepseek-flash",
            &format!(
                r#","context_window":{{"context_window_size":200000,{}}}"#,
                USAGE
            ),
        );
        assert_eq!(resolve_context(&input, &ModelConfig::default()).1, 200_000);
    }

    #[test]
    fn null_current_usage_reads_only_this_sessions_transcript() {
        let dir = std::env::temp_dir().join(format!("ccline-context-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let usage = |tokens: u32| {
            format!(
                r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":{},"output_tokens":0}}}}}}"#,
                tokens
            ) + "\n"
        };
        fs::write(dir.join("other.jsonl"), usage(350_000)).unwrap();
        fs::write(dir.join("resumed.jsonl"), usage(1_000)).unwrap();
        let session = |name: &str, context_window: &str| -> InputData {
            let json = format!(
                r#"{{"model":{{"id":"claude-opus-5-5","display_name":"Opus"}},"workspace":{{"current_dir":"/tmp"}},"transcript_path":"{}"{}}}"#,
                dir.join(name).display(),
                context_window
            );
            serde_json::from_str(&json).unwrap()
        };
        let null_usage = r#","context_window":{"context_window_size":200000,"current_usage":null}"#;
        let models = ModelConfig::default();

        // A new session has no transcript yet, and other sessions say nothing about it
        assert_eq!(
            resolve_context(&session("new.jsonl", null_usage), &models).0,
            None
        );
        // A resumed session before its first response
        assert_eq!(
            resolve_context(&session("resumed.jsonl", null_usage), &models).0,
            Some(1_000)
        );
        // Versions without context_window keep the project history fallback
        assert!(resolve_context(&session("new.jsonl", ""), &models)
            .0
            .is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_from_before_compact_does_not_count() {
        let dir = std::env::temp_dir().join(format!("ccline-compact-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let response = |tokens: u32| {
            format!(
                r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":{},"output_tokens":0}}}}}}"#,
                tokens
            )
        };
        // The last response, then what /compact appends: a boundary and the summary
        let compacted = [
            response(180_000),
            r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}"#
                .to_string(),
            r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"Summary"}}"#
                .to_string(),
        ]
        .join("\n");
        let json = format!(
            r#"{{"model":{{"id":"claude-opus-5-5","display_name":"Opus"}},"workspace":{{"current_dir":"/tmp"}},"transcript_path":"{}","context_window":{{"context_window_size":200000,"current_usage":null}}}}"#,
            path.display()
        );
        let input: InputData = serde_json::from_str(&json).unwrap();
        let models = ModelConfig::default();

        fs::write(&path, format!("{}\n", compacted)).unwrap();
        assert_eq!(resolve_context(&input, &models).0, None);
        // A session resumed after its first response since /compact
        fs::write(&path, format!("{}\n{}\n", compacted, response(30_000))).unwrap();
        assert_eq!(resolve_context(&input, &models).0, Some(30_000));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_native_data_uses_model_config() {
        let config = ModelConfig::default();
        assert_eq!(
            resolve_context(&input("claude-opus-5-5[1m]", ""), &config).1,
            1_000_000
        );
        assert_eq!(
            resolve_context(&input("claude-opus-5-5", ""), &config).1,
            200_000
        );
    }

    #[test]
    fn malformed_context_window_is_ignored() {
        let input = input(
            "claude-opus-5-5",
            r#","context_window":{"context_window_size":"large"}"#,
        );
        assert!(input.context_window.is_none());
    }
}
