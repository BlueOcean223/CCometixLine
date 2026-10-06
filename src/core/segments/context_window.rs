use super::{Segment, SegmentData};
use crate::config::{InputData, ModelConfig, SegmentId, TranscriptEntry};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
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
/// `context_window_size`; older versions infer it from the model ID. Claude models
/// use Claude Code's window alone, as it knows their real one. Earlier versions of
/// ccline required `context_limit` even in entries that only rename a model.
///
/// Tokens come from `context_window.current_usage` and count the last response's
/// output, which becomes input on the next request; Claude Code's own
/// `used_percentage` leaves it out. While it is null (before the session's first API
/// response and right after `/compact`), only the session's own transcript can stand
/// in, which has usage when the session was resumed. Usage from before a compaction
/// does not count, so `/compact` shows no usage until the next response. Older Claude
/// Code versions without `context_window` use the transcript fallback, which may also
/// take usage from the project's latest session.
fn resolve_context(input: &InputData, model_config: &ModelConfig) -> (Option<u32>, u32) {
    let native = input.context_window.as_ref();

    let claude_code_limit = native
        .and_then(|cw| cw.context_window_size)
        .filter(|&size| size > 0)
        .unwrap_or_else(|| model_config.get_inferred_context_limit(&input.model.id));
    let model_limit = model_config
        .get_entry_context_limit(&input.model.id)
        .filter(|_| !input.model.id.to_lowercase().contains("claude"));
    let context_limit = match model_limit {
        Some(model_limit) => model_limit.min(claude_code_limit),
        None => claude_code_limit,
    };

    let tokens = match native {
        Some(cw) => match &cw.current_usage {
            Some(usage) => Some(usage.clone().normalize().display_tokens()),
            None => last_usage(Path::new(&input.transcript_path)).flatten(),
        },
        None => parse_transcript_usage(&input.transcript_path),
    };

    (tokens, context_limit)
}

impl Segment for ContextWindowSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let (context_used_token_opt, context_limit) = resolve_context(input, ModelConfig::load());

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

/// Usage for Claude Code versions without `context_window`: from the transcript, or
/// from the project's latest session while the transcript does not exist yet
fn parse_transcript_usage<P: AsRef<Path>>(transcript_path: P) -> Option<u32> {
    let path = transcript_path.as_ref();
    if path.exists() {
        last_usage_or_summary(path).flatten()
    } else {
        try_find_usage_from_project_history(path)
    }
}

/// Usage of the transcript's last response; `Some(None)` when `/compact` replaced the
/// context after it, and `None` when the transcript has neither.
fn last_usage(path: &Path) -> Option<Option<u32>> {
    find_from_end(path, |line| {
        let entry: TranscriptEntry = serde_json::from_str(line).ok()?;
        match entry.r#type.as_deref()? {
            // Usage from before it describes the context that /compact replaced
            "system" if entry.subtype.as_deref() == Some("compact_boundary") => Some(None),
            "assistant" => Some(Some(entry.message?.usage?.normalize().display_tokens())),
            _ => None,
        }
    })
}

/// Like [`last_usage`], except that a transcript ending in a summary, which older
/// Claude Code versions wrote, gives the usage of the response the summary points to
fn last_usage_or_summary(path: &Path) -> Option<Option<u32>> {
    let last = find_from_end(path, |line| {
        Some(serde_json::from_str::<TranscriptEntry>(line).ok())
    })
    .flatten();
    if let Some(entry) = last.filter(|entry| entry.r#type.as_deref() == Some("summary")) {
        if let Some(leaf_uuid) = &entry.leaf_uuid {
            return find_usage_by_leaf_uuid(leaf_uuid, path.parent()?).map(Some);
        }
    }
    last_usage(path)
}

/// Calls `visit` on the file's non-empty lines, trimmed, from the last one back, until
/// it returns a value. The file is read in blocks from the end, as the lines needed
/// are usually the last ones of a long transcript.
fn find_from_end<T>(path: &Path, visit: impl FnMut(&str) -> Option<T>) -> Option<T> {
    find_from_end_in_blocks(path, 64 * 1024, visit)
}

fn find_from_end_in_blocks<T>(
    path: &Path,
    first_block: u64,
    mut visit: impl FnMut(&str) -> Option<T>,
) -> Option<T> {
    let mut file = fs::File::open(path).ok()?;
    let mut end = file.metadata().ok()?.len();
    let mut block_size = first_block.max(1);
    // The part of a line that starts before the bytes read so far
    let mut rest = Vec::new();
    while end > 0 {
        let start = end.saturating_sub(block_size);
        let mut bytes = vec![0; (end - start) as usize];
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut bytes).ok()?;
        bytes.append(&mut rest);

        // Unless the block starts the file, its first line may start in an earlier one
        let complete = if start == 0 {
            0
        } else {
            bytes
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |newline| newline + 1)
        };
        for line in bytes[complete..].rsplit(|&byte| byte == b'\n') {
            let line = String::from_utf8_lossy(line);
            let line = line.trim();
            if !line.is_empty() {
                if let Some(found) = visit(line) {
                    return Some(found);
                }
            }
        }

        bytes.truncate(complete);
        rest = bytes;
        end = start;
        // Larger blocks keep a long line from being copied once per block
        block_size = block_size.saturating_mul(2);
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

    // Usage from the most recent session that has any, or none when /compact replaced
    // that session's context since
    session_files
        .iter()
        .find_map(|session_path| last_usage_or_summary(session_path))
        .flatten()
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
    fn claude_models_use_claude_codes_window() {
        // An entry written for an earlier version, which required context_limit
        let mut config = ModelConfig::default();
        let user: ModelConfig = toml::from_str(
            r#"
            [[models]]
            pattern = "claude-sonnet-4-5"
            display_name = "Sonnet 4.5"
            context_limit = 200000
            "#,
        )
        .unwrap();
        config.model_entries.splice(0..0, user.model_entries);
        let input = input(
            "claude-sonnet-4-5-20250929[1m]",
            &format!(
                r#","context_window":{{"context_window_size":1000000,{}}}"#,
                USAGE
            ),
        );
        assert_eq!(resolve_context(&input, &config).1, 1_000_000);
    }

    #[test]
    fn project_history_stops_at_a_compacted_session() {
        let dir = std::env::temp_dir().join(format!("ccline-history-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let response = |tokens: u32| {
            format!(
                r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":{},"output_tokens":0}}}}}}"#,
                tokens
            ) + "\n"
        };
        let older = dir.join("older.jsonl");
        let newer = dir.join("newer.jsonl");
        fs::write(&older, response(50_000)).unwrap();
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
        fs::write(
            &newer,
            response(180_000) + r#"{"type":"system","subtype":"compact_boundary"}"# + "\n",
        )
        .unwrap();

        // Versions without context_window, before the new session's transcript exists
        let new_session = dir.join("new.jsonl");
        assert_eq!(parse_transcript_usage(&new_session), None);
        fs::write(&newer, response(180_000)).unwrap();
        assert_eq!(parse_transcript_usage(&new_session), Some(180_000));

        let _ = fs::remove_dir_all(&dir);
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
        fs::write(
            dir.join("other.jsonl"),
            r#"{"type":"assistant","uuid":"u1","message":{"usage":{"input_tokens":350000,"output_tokens":0}}}"#,
        )
        .unwrap();
        fs::write(dir.join("resumed.jsonl"), usage(1_000)).unwrap();
        // How older versions ended a transcript, pointing to the other session's response
        fs::write(
            dir.join("summarized.jsonl"),
            r#"{"type":"summary","summary":"Earlier work","leafUuid":"u1"}"#,
        )
        .unwrap();
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

        // A new session has no transcript yet, and other sessions' transcripts are not used
        assert_eq!(
            resolve_context(&session("new.jsonl", null_usage), &models).0,
            None
        );
        // A resumed session before its first response
        assert_eq!(
            resolve_context(&session("resumed.jsonl", null_usage), &models).0,
            Some(1_000)
        );
        assert_eq!(
            resolve_context(&session("summarized.jsonl", null_usage), &models).0,
            None
        );
        // Versions without context_window keep the summary and project history fallbacks
        assert_eq!(
            resolve_context(&session("summarized.jsonl", ""), &models).0,
            Some(350_000)
        );
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
    fn lines_are_read_from_the_end_across_blocks() {
        let dir = std::env::temp_dir().join(format!("ccline-lines-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lines.jsonl");
        let lines_from_end = |contents: &str, block_size: u64| {
            fs::write(&path, contents).unwrap();
            let mut lines = Vec::new();
            find_from_end_in_blocks(&path, block_size, |line| {
                lines.push(line.to_string());
                None::<()>
            });
            lines
        };

        for block_size in 1..=8 {
            // Lines longer than a block, and a character split between two blocks
            assert_eq!(
                lines_from_end("first line\nsé\n\n third \n", block_size),
                ["third", "sé", "first line"]
            );
            // The last line may lack its newline
            assert_eq!(lines_from_end("a\nbb", block_size), ["bb", "a"]);
            assert!(lines_from_end("", block_size).is_empty());
        }

        // Reading stops at the first line that gives a value
        fs::write(&path, "1\n2\n3\n").unwrap();
        let mut visited = 0;
        let found = find_from_end_in_blocks(&path, 1, |line| {
            visited += 1;
            (line == "2").then_some(line.len())
        });
        assert_eq!((found, visited), (Some(1), 2));

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
