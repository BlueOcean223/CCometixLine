//! Session cost computed from transcripts with ccline's own model prices.
//!
//! Claude Code prices models it does not know at Claude Opus rates, which is far off
//! for third-party models. When the current model has prices configured, the cost
//! segment sums every API response in the session's transcripts instead. Requests
//! Claude Code does not record in transcripts, such as session title generation,
//! are not included.

use crate::config::{ModelConfig, Pricing, TokenUsage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const CACHE_VERSION: u32 = 1;
/// Scan caches untouched for this long are deleted when a new session starts
const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// One API response. Claude Code writes a transcript line per content block, each
/// repeating the response's usage, so responses are keyed by message ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Response {
    model: String,
    timestamp: Option<DateTime<Utc>>,
    usage: TokenUsage,
}

/// Responses read so far and how far each transcript file has been read, so that
/// each render only parses lines appended since the previous one.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ScanCache {
    version: u32,
    offsets: HashMap<String, u64>,
    responses: HashMap<String, Response>,
}

/// Total cost of the session in `currency`, or `None` when a response has no price
/// in that currency.
pub fn session_cost(transcript_path: &Path, models: &ModelConfig, currency: &str) -> Option<f64> {
    let files = transcript_files(transcript_path);
    let cache_path = cache_path(transcript_path);
    let mut cache = cache_path
        .as_deref()
        .and_then(load_cache)
        .unwrap_or_default();
    if scan(&files, &mut cache) {
        if let Some(path) = &cache_path {
            save_cache(path, &cache);
        }
    }
    price(&cache.responses, models, currency)
}

/// The main transcript and its subagents' transcripts in `<session>/subagents/`.
fn transcript_files(transcript_path: &Path) -> Vec<PathBuf> {
    let mut files = vec![transcript_path.to_path_buf()];
    if let Ok(entries) = fs::read_dir(transcript_path.with_extension("").join("subagents")) {
        files.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl")),
        );
    }
    files
}

/// Read new lines from each file. Returns whether the cache changed.
fn scan(files: &[PathBuf], cache: &mut ScanCache) -> bool {
    let offset = |cache: &ScanCache, path: &Path| {
        cache
            .offsets
            .get(path.to_string_lossy().as_ref())
            .copied()
            .unwrap_or(0)
    };
    // A file shorter than what was read has been rewritten: start over
    let rewritten = files
        .iter()
        .any(|path| fs::metadata(path).map_or(0, |m| m.len()) < offset(cache, path));

    let mut changed = false;
    if cache.version != CACHE_VERSION || rewritten {
        *cache = ScanCache {
            version: CACHE_VERSION,
            ..Default::default()
        };
        changed = true;
    }

    for path in files {
        let start = offset(cache, path);
        if let Some(end) = scan_file(path, start, &mut cache.responses) {
            if end != start {
                cache
                    .offsets
                    .insert(path.to_string_lossy().into_owned(), end);
                changed = true;
            }
        }
    }
    changed
}

/// Read the complete lines after `offset`; returns the offset after the last one.
/// A trailing line without a newline is still being written and is read next time.
fn scan_file(path: &Path, offset: u64, responses: &mut HashMap<String, Response>) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut end = offset;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).ok()?;
        if read == 0 || line.last() != Some(&b'\n') {
            break;
        }
        end += read as u64;
        if let Some((id, response)) = parse_response(&line) {
            responses.insert(id, response);
        }
    }
    Some(end)
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    uuid: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_1h_input_tokens: Option<u64>,
}

fn parse_response(line: &[u8]) -> Option<(String, Response)> {
    let line = std::str::from_utf8(line).ok()?;
    // Most lines are not API responses; skip parsing those
    if !line.contains("\"usage\"") {
        return None;
    }
    let entry: Entry = serde_json::from_str(line).ok()?;
    if entry.kind.as_deref() != Some("assistant") {
        return None;
    }
    let message = entry.message?;
    let usage = message.usage?;

    let cache_write = usage.cache_creation_input_tokens.unwrap_or(0);
    let cache_write_1h = usage
        .cache_creation
        .and_then(|c| c.ephemeral_1h_input_tokens)
        .unwrap_or(0)
        .min(cache_write);
    let id = message.id.or(entry.uuid)?;
    Some((
        id,
        Response {
            model: message.model.unwrap_or_default(),
            timestamp: entry.timestamp,
            usage: TokenUsage {
                input: usage.input_tokens.unwrap_or(0),
                output: usage.output_tokens.unwrap_or(0),
                cache_read: usage.cache_read_input_tokens.unwrap_or(0),
                cache_write_5m: cache_write - cache_write_1h,
                cache_write_1h,
            },
        },
    ))
}

/// Sum the responses' costs; `None` if a billable response has no price in `currency`.
fn price(
    responses: &HashMap<String, Response>,
    models: &ModelConfig,
    currency: &str,
) -> Option<f64> {
    let mut pricing_by_model: HashMap<&str, Option<&Pricing>> = HashMap::new();
    let mut total = 0.0;
    for response in responses.values().filter(|r| !r.usage.is_empty()) {
        let pricing = *pricing_by_model
            .entry(response.model.as_str())
            .or_insert_with(|| models.get_pricing(&response.model));
        let pricing = pricing.filter(|p| p.currency == currency)?;
        total += pricing.cost(&response.usage, response.timestamp);
    }
    Some(total)
}

fn cache_path(transcript_path: &Path) -> Option<PathBuf> {
    let session = transcript_path.file_stem()?;
    Some(
        dirs::home_dir()?
            .join(".claude")
            .join("ccline")
            .join("cache")
            .join("costs")
            .join(session)
            .with_extension("json"),
    )
}

fn load_cache(path: &Path) -> Option<ScanCache> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn save_cache(path: &Path, cache: &ScanCache) {
    let Some(dir) = path.parent() else {
        return;
    };
    if !path.exists() {
        prune_old_caches(dir);
    }
    let _ = fs::create_dir_all(dir);
    let Ok(json) = serde_json::to_vec(cache) else {
        return;
    };
    // Write then rename, so a concurrent render never reads a partial file
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

fn prune_old_caches(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let expired = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > CACHE_MAX_AGE);
        if expired {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(id: &str, model: &str, timestamp: &str, usage: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{}","message":{{"id":"{}","model":"{}","usage":{}}}}}"#,
            timestamp, id, model, usage
        ) + "\n"
    }

    const USAGE: &str = r#"{"input_tokens":248,"cache_creation_input_tokens":0,"cache_read_input_tokens":11904,"output_tokens":2}"#;

    #[test]
    fn dedupes_lines_of_one_response() {
        let dir = std::env::temp_dir().join(format!("ccline-cost-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("session").join("subagents")).unwrap();
        let main = dir.join("session.jsonl");
        // Two content blocks of one response, one user line, one subagent response
        let monday_peak = "2026-10-12T02:00:00Z";
        let content = line("a", "deepseek-flash", monday_peak, USAGE).repeat(2)
            + r#"{"type":"user","message":{"content":"hi"}}"#
            + "\n";
        fs::write(&main, content).unwrap();
        fs::write(
            dir.join("session").join("subagents").join("agent-1.jsonl"),
            line("b", "deepseek-flash", monday_peak, USAGE),
        )
        .unwrap();

        let mut cache = ScanCache::default();
        assert!(scan(&transcript_files(&main), &mut cache));
        assert_eq!(cache.responses.len(), 2);

        let models = ModelConfig::default();
        let one = (248.0 * 2.0 + 2.0 * 8.0 + 11_904.0 * 0.04) / 1e6;
        let total = price(&cache.responses, &models, "¥").unwrap();
        assert!((total - 2.0 * one).abs() < 1e-12);

        // Only appended lines are read; a partial line waits for its newline
        let mut file = fs::OpenOptions::new().append(true).open(&main).unwrap();
        file.write_all(line("c", "deepseek-flash", monday_peak, USAGE).as_bytes())
            .unwrap();
        file.write_all(br#"{"type":"assistant","#).unwrap();
        assert!(scan(&transcript_files(&main), &mut cache));
        assert_eq!(cache.responses.len(), 3);
        assert!(!scan(&transcript_files(&main), &mut cache));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpriced_model_or_other_currency_gives_none() {
        let models = ModelConfig::default();
        let mut responses = HashMap::new();
        let usage = TokenUsage {
            input: 10,
            ..Default::default()
        };
        responses.insert(
            "a".to_string(),
            Response {
                model: "deepseek-flash".to_string(),
                timestamp: None,
                usage,
            },
        );
        assert!(price(&responses, &models, "$").is_none());
        responses.insert(
            "b".to_string(),
            Response {
                model: "some-unknown-model".to_string(),
                timestamp: None,
                usage,
            },
        );
        assert!(price(&responses, &models, "¥").is_none());
    }
}
