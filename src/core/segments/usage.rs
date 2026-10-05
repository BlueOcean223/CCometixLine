use super::{join_details, Segment, SegmentData};
use crate::config::{InputData, RateLimitWindow, SegmentId};
use crate::utils::credentials;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct ApiUsageResponse {
    five_hour: UsagePeriod,
    seven_day: UsagePeriod,
}

#[derive(Debug, Deserialize)]
struct UsagePeriod {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApiUsageCache {
    five_hour_utilization: f64,
    seven_day_utilization: f64,
    /// RFC 3339
    #[serde(default)]
    five_hour_resets_at: Option<String>,
    cached_at: String,
}

/// Utilization of both windows, 0 to 100, and when the five-hour window resets
#[derive(Debug, PartialEq)]
struct UsageSnapshot {
    five_hour: f64,
    seven_day: f64,
    five_hour_resets_at: Option<DateTime<Utc>>,
}

pub struct UsageSegment {
    show_reset_time: bool,
    show_seven_day: bool,
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
        }
    }

    pub fn with_reset_time(mut self, show_reset_time: bool) -> Self {
        self.show_reset_time = show_reset_time;
        self
    }

    pub fn with_seven_day(mut self, show_seven_day: bool) -> Self {
        self.show_seven_day = show_seven_day;
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

        Some(UsageSnapshot {
            five_hour: utilization(&limits.five_hour),
            seven_day: utilization(&limits.seven_day),
            five_hour_resets_at: limits
                .five_hour
                .as_ref()
                .and_then(|w| w.resets_at)
                .and_then(|ts| DateTime::from_timestamp(ts, 0)),
        })
    }

    fn get_cache_path() -> Option<std::path::PathBuf> {
        let home = dirs::home_dir()?;
        Some(
            home.join(".claude")
                .join("ccline")
                .join(".api_usage_cache.json"),
        )
    }

    fn load_cache(&self) -> Option<ApiUsageCache> {
        let cache_path = Self::get_cache_path()?;
        if !cache_path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&cache_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save_cache(&self, cache: &ApiUsageCache) {
        if let Some(cache_path) = Self::get_cache_path() {
            if let Some(parent) = cache_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(cache) {
                let _ = std::fs::write(&cache_path, json);
            }
        }
    }

    fn is_cache_valid(&self, cache: &ApiUsageCache, cache_duration: u64) -> bool {
        if let Ok(cached_at) = DateTime::parse_from_rfc3339(&cache.cached_at) {
            let now = Utc::now();
            let elapsed = now.signed_duration_since(cached_at.with_timezone(&Utc));
            elapsed.num_seconds() < cache_duration as i64
        } else {
            false
        }
    }

    fn get_proxy_from_settings() -> Option<String> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()?;
        let settings_path = format!("{}/.claude/settings.json", home);

        let content = std::fs::read_to_string(&settings_path).ok()?;
        let settings: serde_json::Value = serde_json::from_str(&content).ok()?;

        // Try HTTPS_PROXY first, then HTTP_PROXY
        settings
            .get("env")?
            .get("HTTPS_PROXY")
            .or_else(|| settings.get("env")?.get("HTTP_PROXY"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn fetch_api_usage(
        &self,
        api_base_url: &str,
        token: &str,
        timeout_secs: u64,
        claude_code_version: Option<&str>,
    ) -> Option<ApiUsageResponse> {
        let url = format!("{}/api/oauth/usage", api_base_url);
        let user_agent = match claude_code_version {
            Some(version) => format!("claude-code/{}", version),
            None => "claude-code".to_string(),
        };

        let agent = if let Some(proxy_url) = Self::get_proxy_from_settings() {
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
            .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
            .build()
            .call()
            .ok()?;

        response.into_body().read_json().ok()
    }

    /// Query the OAuth usage API, with a file cache. Only needed when Claude Code
    /// does not report `rate_limits` (before the first API response, or older versions).
    fn usage_from_api(&self, claude_code_version: Option<&str>) -> Option<UsageSnapshot> {
        let token = credentials::get_oauth_token()?;

        // Load config from file to get segment options
        let config = crate::config::Config::load().ok()?;
        let segment_config = config.segments.iter().find(|s| s.id == SegmentId::Usage)?;
        let api_base_url = segment_config.option("api_base_url");
        let api_base_url = api_base_url.as_str().unwrap_or_default();
        let cache_duration = segment_config
            .option("cache_duration")
            .as_u64()
            .unwrap_or_default();
        let timeout = segment_config
            .option("timeout")
            .as_u64()
            .unwrap_or_default();

        let cache = match self.load_cache() {
            Some(cache) if self.is_cache_valid(&cache, cache_duration) => cache,
            cached_data => {
                match self.fetch_api_usage(api_base_url, &token, timeout, claude_code_version) {
                    Some(response) => {
                        let cache = ApiUsageCache {
                            five_hour_utilization: response.five_hour.utilization,
                            seven_day_utilization: response.seven_day.utilization,
                            five_hour_resets_at: response.five_hour.resets_at,
                            cached_at: Utc::now().to_rfc3339(),
                        };
                        self.save_cache(&cache);
                        cache
                    }
                    None => cached_data?,
                }
            }
        };

        Some(UsageSnapshot {
            five_hour: cache.five_hour_utilization,
            seven_day: cache.seven_day_utilization,
            five_hour_resets_at: cache
                .five_hour_resets_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
        })
    }

    /// The five-hour window's usage, with the icon showing the same window and its
    /// reset time, followed by the seven-day window's usage.
    fn segment_data(&self, usage: &UsageSnapshot) -> SegmentData {
        let primary = format!("{}%", usage.five_hour.round() as u8);
        let mut details = Vec::new();
        if let Some(resets_at) = usage.five_hour_resets_at.filter(|_| self.show_reset_time) {
            details.push(resets_at.with_timezone(&Local).format("%H:%M").to_string());
        }
        if self.show_seven_day {
            details.push(format!("7d {}%", usage.seven_day.round() as u8));
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
            Some(usage) => usage,
            None => self.usage_from_api(input.version.as_deref())?,
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
