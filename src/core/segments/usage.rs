use super::{join_details, Segment, SegmentData};
use crate::config::{InputData, RateLimitWindow, SegmentId};
use crate::utils::claude_settings::ClaudeSettings;
use crate::utils::credentials;
use crate::utils::files::write_atomic;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Cached usage stays on show for this many `cache_duration`s, so that a failed request
/// or two keep the last answer, and longer failures hide it rather than show old usage
const STALE_AFTER_REFRESHES: u64 = 3;
/// Longer timeouts are cut to this; ureq panics when the deadline overflows
const MAX_TIMEOUT_SECS: u64 = 3600;

/// The API's fields are nullable
#[derive(Debug, Deserialize)]
struct ApiUsageResponse {
    five_hour: Option<UsagePeriod>,
    seven_day: Option<UsagePeriod>,
    /// Limits beyond the two windows, among them the weekly window of each model
    /// with a limit of its own. Entries vary in shape, so they are read one by one
    limits: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct UsagePeriod {
    utilization: Option<f64>,
    /// RFC 3339
    resets_at: Option<String>,
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Whole seconds from `since` to `now`, or `None` when `since` is later, as after the
/// clock is set back
fn elapsed(since: DateTime<Utc>, now: DateTime<Utc>) -> Option<u64> {
    u64::try_from((now - since).num_seconds()).ok()
}

impl ApiUsageResponse {
    /// The weekly window of the Fable models, an entry of `limits` such as
    /// `{"kind":"weekly_scoped","scope":{"model":{"display_name":"Fable"}},"percent":12.0}`
    fn fable(&self) -> Option<ModelUsage> {
        let limits = self.limits.as_ref().and_then(|limits| limits.as_array());
        limits.into_iter().flatten().find_map(|limit| {
            let name = limit.pointer("/scope/model/display_name")?.as_str()?;
            if limit.get("kind")?.as_str()? != "weekly_scoped"
                || !name.to_lowercase().starts_with("fable")
            {
                return None;
            }
            // Epoch seconds or RFC 3339
            let resets_at = match limit.get("resets_at") {
                Some(serde_json::Value::Number(ts)) => {
                    ts.as_i64().and_then(|ts| DateTime::from_timestamp(ts, 0))
                }
                Some(serde_json::Value::String(s)) => parse_rfc3339(s),
                _ => None,
            };
            Some(ModelUsage {
                name: name.to_string(),
                utilization: limit.get("percent")?.as_f64()?,
                resets_at,
            })
        })
    }

    /// The usage the response reports, or `None` when it reports neither window, as
    /// an error body from a relay may
    fn snapshot(self) -> Option<UsageSnapshot> {
        let fable = self.fable();
        let window = |period: Option<UsagePeriod>| {
            let period = period?;
            let resets_at = period.resets_at.as_deref().and_then(parse_rfc3339);
            Some((period.utilization?, resets_at))
        };
        let five_hour = window(self.five_hour);
        let seven_day = window(self.seven_day);
        if five_hour.is_none() && seven_day.is_none() {
            return None;
        }

        // As on stdin, a missing window has nothing used yet
        let (five_hour, five_hour_resets_at) = five_hour.unwrap_or((0.0, None));
        let (seven_day, seven_day_resets_at) = seven_day.unwrap_or((0.0, None));
        Some(UsageSnapshot {
            five_hour,
            seven_day,
            five_hour_resets_at,
            seven_day_resets_at,
            fable,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ApiUsageCache {
    /// When the API was last asked, whether or not it answered
    #[serde(default)]
    attempted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    usage: Option<CachedUsage>,
}

/// The usage API's last answer
#[derive(Debug, Serialize, Deserialize)]
struct CachedUsage {
    fetched_at: DateTime<Utc>,
    #[serde(flatten)]
    usage: UsageSnapshot,
}

impl CachedUsage {
    /// The usage to show at `now`: none once the API has not confirmed it for several
    /// refreshes, and nothing used in a window whose reset time has passed, as Claude
    /// Code reports a window once it resets
    fn at(self, now: DateTime<Utc>, cache_duration: u64) -> Option<UsageSnapshot> {
        let age = elapsed(self.fetched_at, now)?;
        if age > STALE_AFTER_REFRESHES.saturating_mul(cache_duration) {
            return None;
        }

        let reset = |utilization: &mut f64, resets_at: &mut Option<DateTime<Utc>>| {
            if resets_at.is_some_and(|at| at <= now) {
                *utilization = 0.0;
                *resets_at = None;
            }
        };
        let mut usage = self.usage;
        reset(&mut usage.five_hour, &mut usage.five_hour_resets_at);
        reset(&mut usage.seven_day, &mut usage.seven_day_resets_at);
        if let Some(fable) = &mut usage.fable {
            reset(&mut fable.utilization, &mut fable.resets_at);
        }
        Some(usage)
    }
}

/// The weekly window of a model with a limit of its own
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ModelUsage {
    /// The name the usage API gives the model, such as `Fable`
    name: String,
    /// 0 to 100
    utilization: f64,
    resets_at: Option<DateTime<Utc>>,
}

/// Utilization of both windows, 0 to 100, and when they reset
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct UsageSnapshot {
    five_hour: f64,
    seven_day: f64,
    five_hour_resets_at: Option<DateTime<Utc>>,
    seven_day_resets_at: Option<DateTime<Utc>>,
    /// Only the usage API reports it
    fable: Option<ModelUsage>,
}

pub struct UsageSegment {
    show_reset_time: bool,
    show_seven_day: bool,
    show_seven_day_reset: bool,
    show_fable: bool,
    api_base_url: String,
    /// Seconds between requests to the usage API
    cache_duration: u64,
    /// Seconds to wait for the usage API
    timeout: u64,
}

impl Default for UsageSegment {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageSegment {
    pub fn new() -> Self {
        Self {
            show_reset_time: true,
            show_seven_day: true,
            show_seven_day_reset: false,
            show_fable: false,
            api_base_url: "https://api.anthropic.com".to_string(),
            cache_duration: 180,
            timeout: 2,
        }
    }

    /// The usage API's base URL, and how many seconds to cache its answers and to wait
    /// for one
    pub fn with_api(mut self, base_url: &str, cache_duration: u64, timeout: u64) -> Self {
        self.api_base_url = base_url.to_string();
        self.cache_duration = cache_duration;
        self.timeout = timeout;
        self
    }

    pub fn with_reset_time(mut self, show_reset_time: bool) -> Self {
        self.show_reset_time = show_reset_time;
        self
    }

    pub fn with_seven_day(mut self, show_seven_day: bool) -> Self {
        self.show_seven_day = show_seven_day;
        self
    }

    pub fn with_seven_day_reset(mut self, show_seven_day_reset: bool) -> Self {
        self.show_seven_day_reset = show_seven_day_reset;
        self
    }

    pub fn with_fable(mut self, show_fable: bool) -> Self {
        self.show_fable = show_fable;
        self
    }

    fn get_circle_icon(utilization: f64) -> String {
        let percent = (utilization * 100.0) as u8;
        match percent {
            0..=12 => "\u{f0a9e}".to_string(),  // circle_slice_1
            13..=25 => "\u{f0a9f}".to_string(), // circle_slice_2
            26..=37 => "\u{f0aa0}".to_string(), // circle_slice_3
            38..=50 => "\u{f0aa1}".to_string(), // circle_slice_4
            51..=62 => "\u{f0aa2}".to_string(), // circle_slice_5
            63..=75 => "\u{f0aa3}".to_string(), // circle_slice_6
            76..=87 => "\u{f0aa4}".to_string(), // circle_slice_7
            _ => "\u{f0aa5}".to_string(),       // circle_slice_8
        }
    }

    /// Rate limits reported by Claude Code on stdin.
    fn usage_from_input(input: &InputData) -> Option<UsageSnapshot> {
        let limits = input.rate_limits.as_ref()?;
        if limits.five_hour.is_none() && limits.seven_day.is_none() {
            return None;
        }

        // Claude Code drops a window once it resets, so a missing window has nothing used yet
        let utilization =
            |window: &Option<RateLimitWindow>| window.as_ref().map_or(0.0, |w| w.used_percentage);
        let resets_at = |window: &Option<RateLimitWindow>| {
            window
                .as_ref()
                .and_then(|w| w.resets_at)
                .and_then(|ts| DateTime::from_timestamp(ts, 0))
        };

        Some(UsageSnapshot {
            five_hour: utilization(&limits.five_hour),
            seven_day: utilization(&limits.seven_day),
            five_hour_resets_at: resets_at(&limits.five_hour),
            seven_day_resets_at: resets_at(&limits.seven_day),
            fable: None,
        })
    }

    fn cache_dir() -> Option<PathBuf> {
        Some(dirs::home_dir()?.join(".claude").join("ccline"))
    }

    /// The usage API's cache file in `dir`, shared by the sessions of an account since
    /// they share the same limits. An account in another Claude Code configuration
    /// directory has its own.
    fn cache_file(dir: &Path) -> PathBuf {
        dir.join(format!(
            ".api_usage_cache{}.json",
            credentials::account_suffix()
        ))
    }

    fn load_cache(dir: &Path) -> Option<ApiUsageCache> {
        let content = std::fs::read_to_string(Self::cache_file(dir)).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save_cache(dir: &Path, cache: &ApiUsageCache) {
        if let Ok(json) = serde_json::to_string_pretty(cache) {
            let _ = write_atomic(&Self::cache_file(dir), json.as_bytes());
        }
    }

    /// Load the cache, and if the API was last asked `cache_duration` or more ago, record
    /// that this process asks it now. Sessions take turns under a file lock, so of the
    /// sessions that render at the same moment, as all of them do when a limit resets,
    /// only one asks.
    fn claim_request(dir: &Path, cache_duration: u64) -> (ApiUsageCache, bool) {
        // Released when the file closes, also when Claude Code kills the process.
        // Where locking fails, sessions go on without it
        let _lock = Self::lock_cache(dir);
        let mut cache = Self::load_cache(dir).unwrap_or_default();
        let now = Utc::now();
        let asked_recently = cache
            .attempted_at
            .and_then(|at| elapsed(at, now))
            .is_some_and(|seconds| seconds < cache_duration);
        if asked_recently {
            return (cache, false);
        }
        cache.attempted_at = Some(now);
        Self::save_cache(dir, &cache);
        (cache, true)
    }

    /// Held while a session checks and records when the API was last asked
    fn lock_cache(dir: &Path) -> Option<File> {
        std::fs::create_dir_all(dir).ok()?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(Self::cache_file(dir).with_extension("lock"))
            .ok()?;
        file.lock().ok()?;
        Some(file)
    }

    /// The proxy in the `env` of the Claude Code settings that apply to the project
    fn get_proxy_from_settings(project_dir: Option<&Path>) -> Option<String> {
        let settings = ClaudeSettings::load(project_dir);
        // Try HTTPS_PROXY first, then HTTP_PROXY
        settings
            .get(&["env", "HTTPS_PROXY"])
            .or_else(|| settings.get(&["env", "HTTP_PROXY"]))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn fetch_api_usage(&self, token: &str, input: &InputData) -> Option<ApiUsageResponse> {
        let url = format!("{}/api/oauth/usage", self.api_base_url);
        let user_agent = match &input.version {
            Some(version) => format!("claude-code/{}", version),
            None => "claude-code".to_string(),
        };

        let project_dir = input.workspace.project_dir.as_deref().map(Path::new);
        let agent = if let Some(proxy_url) = Self::get_proxy_from_settings(project_dir) {
            if let Ok(proxy) = ureq::Proxy::new(&proxy_url) {
                ureq::Agent::config_builder()
                    .proxy(Some(proxy))
                    .build()
                    .new_agent()
            } else {
                ureq::Agent::new_with_defaults()
            }
        } else {
            ureq::Agent::new_with_defaults()
        };

        let response = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", token))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("User-Agent", &user_agent)
            .config()
            .timeout_global(Some(std::time::Duration::from_secs(
                self.timeout.min(MAX_TIMEOUT_SECS),
            )))
            .build()
            .call()
            .ok()?;

        response.into_body().read_json().ok()
    }

    /// Query the OAuth usage API, with a file cache. Only needed when Claude Code
    /// does not report `rate_limits` (before the first API response, or older versions),
    /// or for the Fable window, which Claude Code does not report.
    fn usage_from_api(&self, input: &InputData) -> Option<UsageSnapshot> {
        let dir = Self::cache_dir()?;
        // The attempt is recorded before asking, so that a request that fails, times out,
        // or is cut short when Claude Code cancels the render also waits `cache_duration`
        let (mut cache, ask) = Self::claim_request(&dir, self.cache_duration);
        if ask {
            match credentials::get_oauth_token() {
                Some(token) => {
                    let usage = self
                        .fetch_api_usage(&token, input)
                        .and_then(ApiUsageResponse::snapshot);
                    if let Some(usage) = usage {
                        let usage = CachedUsage {
                            fetched_at: Utc::now(),
                            usage,
                        };
                        Self::store_usage(&dir, &mut cache, Some(usage));
                    }
                }
                // Signed out, or signed in with an API key: the cached usage is that of
                // the account signed in before
                None if cache.usage.is_some() => Self::store_usage(&dir, &mut cache, None),
                None => {}
            }
        }
        cache.usage?.at(Utc::now(), self.cache_duration)
    }

    /// Record the API's answer in the cache claimed before asking. Under the lock, so
    /// that a later request recorded by another session is kept.
    fn store_usage(dir: &Path, cache: &mut ApiUsageCache, usage: Option<CachedUsage>) {
        let _lock = Self::lock_cache(dir);
        let attempted_at = Self::load_cache(dir).and_then(|stored| stored.attempted_at);
        cache.attempted_at = cache.attempted_at.max(attempted_at);
        cache.usage = usage;
        Self::save_cache(dir, cache);
    }

    /// The five-hour window's usage, with the icon showing the same window and its
    /// reset time, followed by the seven-day window's usage and reset time, then the
    /// Fable window's usage.
    fn segment_data(&self, usage: &UsageSnapshot) -> SegmentData {
        let primary = format!("{}%", usage.five_hour.round() as u8);
        let mut details = Vec::new();
        if let Some(resets_at) = usage.five_hour_resets_at.filter(|_| self.show_reset_time) {
            details.push(resets_at.with_timezone(&Local).format("%H:%M").to_string());
        }
        if self.show_seven_day {
            details.push(format!("7d {}%", usage.seven_day.round() as u8));
            if let Some(resets_at) = usage
                .seven_day_resets_at
                .filter(|_| self.show_seven_day_reset)
            {
                details.push(
                    resets_at
                        .with_timezone(&Local)
                        .format("%m-%d %H:%M")
                        .to_string(),
                );
            }
        }
        if let Some(fable) = usage.fable.as_ref().filter(|_| self.show_fable) {
            details.push(format!(
                "{} {}%",
                fable.name,
                fable.utilization.round() as u8
            ));
        }

        let mut metadata = HashMap::new();
        metadata.insert(
            "dynamic_icon".to_string(),
            Self::get_circle_icon(usage.five_hour / 100.0),
        );
        metadata.insert(
            "five_hour_utilization".to_string(),
            usage.five_hour.to_string(),
        );
        metadata.insert(
            "seven_day_utilization".to_string(),
            usage.seven_day.to_string(),
        );
        if let Some(resets_at) = usage.five_hour_resets_at {
            metadata.insert("five_hour_resets_at".to_string(), resets_at.to_rfc3339());
        }
        if let Some(resets_at) = usage.seven_day_resets_at {
            metadata.insert("seven_day_resets_at".to_string(), resets_at.to_rfc3339());
        }
        if let Some(fable) = &usage.fable {
            metadata.insert(
                "fable_utilization".to_string(),
                fable.utilization.to_string(),
            );
        }

        SegmentData {
            primary,
            secondary: join_details(&details),
            metadata,
        }
    }
}

impl Segment for UsageSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let usage = match Self::usage_from_input(input) {
            // The windows Claude Code reports are newer than the API's, which only adds Fable
            Some(usage) if self.show_fable => UsageSnapshot {
                fable: self.usage_from_api(input).and_then(|api| api.fable),
                ..usage
            },
            Some(usage) => usage,
            None => self.usage_from_api(input)?,
        };
        Some(self.segment_data(&usage))
    }

    fn id(&self) -> SegmentId {
        SegmentId::Usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(extra: &str) -> InputData {
        let json = format!(
            r#"{{"model":{{"id":"claude-opus-5-5","display_name":"Opus"}},"workspace":{{"current_dir":"/tmp"}},"transcript_path":"/tmp/t.jsonl"{}}}"#,
            extra
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn reads_rate_limits_from_input() {
        let input = input(
            r#","rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}"#,
        );
        let usage = UsageSegment::usage_from_input(&input).unwrap();
        assert_eq!(
            usage,
            UsageSnapshot {
                five_hour: 23.5,
                seven_day: 41.2,
                five_hour_resets_at: DateTime::from_timestamp(1738425600, 0),
                seven_day_resets_at: DateTime::from_timestamp(1738857600, 0),
                fable: None,
            }
        );

        // Number, icon and reset time all describe the five-hour window
        let data = UsageSegment::new().segment_data(&usage);
        let reset = usage
            .five_hour_resets_at
            .unwrap()
            .with_timezone(&Local)
            .format("%H:%M");
        assert_eq!(data.primary, "24%");
        assert_eq!(data.secondary, format!("· {} · 7d 41%", reset));
        assert_eq!(data.metadata["dynamic_icon"], "\u{f0a9f}");

        let segment = UsageSegment::new().with_reset_time(false);
        assert_eq!(segment.segment_data(&usage).secondary, "· 7d 41%");
        let segment = UsageSegment::new().with_seven_day(false);
        assert_eq!(
            segment.segment_data(&usage).secondary,
            format!("· {}", reset)
        );
        let segment = segment.with_reset_time(false);
        assert_eq!(segment.segment_data(&usage).secondary, "");
    }

    #[test]
    fn seven_day_reset_time_follows_its_usage() {
        let input = input(
            r#","rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}"#,
        );
        let usage = UsageSegment::usage_from_input(&input).unwrap();
        let reset = usage
            .seven_day_resets_at
            .unwrap()
            .with_timezone(&Local)
            .format("%m-%d %H:%M");

        let segment = UsageSegment::new()
            .with_reset_time(false)
            .with_seven_day_reset(true);
        assert_eq!(
            segment.segment_data(&usage).secondary,
            format!("· 7d 41% · {}", reset)
        );
        // Without the usage it belongs to, the reset time is not shown either
        let segment = segment.with_seven_day(false);
        assert_eq!(segment.segment_data(&usage).secondary, "");
    }

    #[test]
    fn reads_fable_from_api_limits() {
        let response: ApiUsageResponse = serde_json::from_str(
            r#"{"five_hour":null,"seven_day":{"utilization":41.0,"resets_at":"2026-10-08T14:00:00+00:00"},"limits":[
                {"kind":"weekly_scoped","scope":{"model":null},"percent":5.0},
                "unexpected",
                {"kind":"daily","scope":{"model":{"display_name":"Fable"}},"percent":80.0},
                {"kind":"weekly_scoped","scope":{"model":{"display_name":"Opus"}},"percent":30.0},
                {"kind":"weekly_scoped","scope":{"model":{"display_name":"Fable"}},"percent":12.4,"resets_at":1791468000}
            ]}"#,
        )
        .unwrap();
        assert!(response.five_hour.is_none());
        assert_eq!(
            response.fable(),
            Some(ModelUsage {
                name: "Fable".to_string(),
                utilization: 12.4,
                resets_at: DateTime::from_timestamp(1791468000, 0),
            })
        );
        let usage = response.snapshot().unwrap();
        assert_eq!(usage.five_hour, 0.0);
        assert_eq!(
            usage.seven_day_resets_at,
            DateTime::from_timestamp(1791468000, 0)
        );

        let response: ApiUsageResponse = serde_json::from_str(
            r#"{"five_hour":{"utilization":24.0,"resets_at":null},"seven_day":{"utilization":41.0,"resets_at":null}}"#,
        )
        .unwrap();
        assert_eq!(response.fable(), None);
    }

    #[test]
    fn api_responses_without_windows_are_not_used() {
        let snapshot = |body: &str| {
            serde_json::from_str::<ApiUsageResponse>(body)
                .unwrap()
                .snapshot()
        };
        assert_eq!(snapshot("{}"), None);
        assert_eq!(snapshot(r#"{"error":{"type":"rate_limit_error"}}"#), None);
        assert_eq!(
            snapshot(r#"{"five_hour":null,"seven_day":{"utilization":null}}"#),
            None
        );

        // Null or unexpected limits leave the windows readable
        for limits in ["null", r#"{"kind":"weekly_scoped"}"#] {
            let body = format!(
                r#"{{"five_hour":{{"utilization":24.0,"resets_at":null}},"seven_day":null,"limits":{}}}"#,
                limits
            );
            let usage = snapshot(&body).unwrap();
            assert_eq!((usage.five_hour, usage.seven_day), (24.0, 0.0));
            assert_eq!(usage.fable, None);
        }
    }

    #[test]
    fn sessions_rendering_together_ask_once() {
        let dir = std::env::temp_dir().join(format!("ccline-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let threads: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || UsageSegment::claim_request(&dir, 180).1)
            })
            .collect();
        let asked = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|&ask| ask)
            .count();
        assert_eq!(asked, 1);
        // Within `cache_duration`, whether or not the API answered
        assert!(!UsageSegment::claim_request(&dir, 180).1);
        // A cache_duration of 0 asks every time
        assert!(UsageSegment::claim_request(&dir, 0).1);

        // A cache file from before `attempted_at` asks the API once
        std::fs::write(
            UsageSegment::cache_file(&dir),
            r#"{"five_hour_utilization":24.0,"seven_day_utilization":41.0,"cached_at":"2026-10-05T12:00:00+00:00"}"#,
        )
        .unwrap();
        assert!(UsageSegment::claim_request(&dir, 180).1);

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{:?}", leftovers);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cached_usage_ages_out_and_resets() {
        let now = Utc::now();
        let seconds = chrono::Duration::seconds;
        let cached = |fetched_ago: i64| CachedUsage {
            fetched_at: now - seconds(fetched_ago),
            usage: UsageSnapshot {
                five_hour: 87.0,
                seven_day: 41.0,
                five_hour_resets_at: Some(now - seconds(60)),
                seven_day_resets_at: Some(now + seconds(3600)),
                fable: Some(ModelUsage {
                    name: "Fable".to_string(),
                    utilization: 12.0,
                    resets_at: Some(now),
                }),
            },
        };

        // Windows past their reset time have nothing used yet
        let usage = cached(10).at(now, 180).unwrap();
        assert_eq!((usage.five_hour, usage.five_hour_resets_at), (0.0, None));
        assert_eq!(usage.seven_day, 41.0);
        assert_eq!(usage.seven_day_resets_at, Some(now + seconds(3600)));
        let fable = usage.fable.unwrap();
        assert_eq!((fable.utilization, fable.resets_at), (0.0, None));

        // Kept through two failed refreshes, hidden after the third
        assert!(cached(540).at(now, 180).is_some());
        assert!(cached(541).at(now, 180).is_none());
        // Fetched after `now`, the clock has been set back
        assert!(cached(-60).at(now, 180).is_none());
    }

    #[test]
    fn fable_usage_is_an_option() {
        let input = input(
            r#","rate_limits":{"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}"#,
        );
        let usage = UsageSnapshot {
            fable: Some(ModelUsage {
                name: "Fable".to_string(),
                utilization: 12.4,
                resets_at: None,
            }),
            ..UsageSegment::usage_from_input(&input).unwrap()
        };
        assert_eq!(
            UsageSegment::new().segment_data(&usage).secondary,
            "· 7d 41%"
        );
        assert_eq!(
            UsageSegment::new()
                .with_fable(true)
                .segment_data(&usage)
                .secondary,
            "· 7d 41% · Fable 12%"
        );
    }

    #[test]
    fn missing_window_counts_as_unused() {
        let input = input(
            r#","rate_limits":{"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}"#,
        );
        let usage = UsageSegment::usage_from_input(&input).unwrap();
        assert_eq!(usage.five_hour, 0.0);
        assert_eq!(usage.five_hour_resets_at, None);
        assert_eq!(
            UsageSegment::new().segment_data(&usage).secondary,
            "· 7d 41%"
        );
    }

    #[test]
    fn no_windows_falls_back() {
        assert!(UsageSegment::usage_from_input(&input("")).is_none());
        assert!(UsageSegment::usage_from_input(&input(r#","rate_limits":{}"#)).is_none());
    }
}
