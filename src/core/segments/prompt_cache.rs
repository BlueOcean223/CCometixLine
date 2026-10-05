use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use chrono::{DateTime, Local};
use std::collections::HashMap;

/// The prompt cache hit ratio Claude Code reports for the main conversation and, for
/// Claude models, when the cache goes cold.
#[derive(Default)]
pub struct PromptCacheSegment;

impl PromptCacheSegment {
    pub fn new() -> Self {
        Self
    }
}

impl Segment for PromptCacheSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let cache = input.prompt_cache.as_ref()?;
        // No response reported cache tokens: caching is off or the provider does not report it
        if !cache.caching_observed {
            return None;
        }
        let hit_ratio = cache.hit_ratio?;
        let expires_at = cache
            .expires_at
            .and_then(|ts| DateTime::from_timestamp(ts, 0));

        // Claude Code derives expiry from Anthropic's cache lifetimes (5 minutes or
        // 1 hour), which other providers' caches do not follow
        let secondary = if !input.model.id.to_lowercase().contains("claude") {
            String::new()
        } else if !cache.warm {
            "· cold".to_string()
        } else {
            expires_at.map_or(String::new(), |time| {
                format!("· {}", time.with_timezone(&Local).format("%H:%M"))
            })
        };

        let mut metadata = HashMap::new();
        metadata.insert("hit_ratio".to_string(), hit_ratio.to_string());
        metadata.insert("warm".to_string(), cache.warm.to_string());
        if let Some(time) = expires_at {
            metadata.insert("expires_at".to_string(), time.to_rfc3339());
        }

        Some(SegmentData {
            primary: format!("{}%", (hit_ratio * 100.0).round() as u8),
            secondary,
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::PromptCache
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(model: &str, prompt_cache: &str) -> Option<SegmentData> {
        let json = format!(
            r#"{{"model":{{"id":"{}","display_name":""}},"workspace":{{"current_dir":"/tmp"}},"transcript_path":"/tmp/t.jsonl","prompt_cache":{}}}"#,
            model, prompt_cache
        );
        PromptCacheSegment::new().collect(&serde_json::from_str(&json).unwrap())
    }

    #[test]
    fn claude_models_show_when_the_cache_goes_cold() {
        let data = collect(
            "claude-opus-5-5",
            r#"{"warm":true,"caching_observed":true,"ttl":"5m","expires_at":1738425600,"hit_ratio":0.917}"#,
        )
        .unwrap();
        let expiry = DateTime::from_timestamp(1738425600, 0)
            .unwrap()
            .with_timezone(&Local)
            .format("%H:%M");
        assert_eq!(data.primary, "92%");
        assert_eq!(data.secondary, format!("· {}", expiry));

        let data = collect(
            "claude-opus-5-5",
            r#"{"warm":false,"caching_observed":true,"expires_at":null,"hit_ratio":0.5}"#,
        )
        .unwrap();
        assert_eq!(data.secondary, "· cold");
    }

    #[test]
    fn other_models_show_only_the_hit_ratio() {
        let data = collect(
            "deepseek-flash[1m]",
            r#"{"warm":true,"caching_observed":true,"expires_at":1738425600,"hit_ratio":0.8}"#,
        )
        .unwrap();
        assert_eq!(data.primary, "80%");
        assert_eq!(data.secondary, "");
    }

    #[test]
    fn hidden_without_cache_data() {
        let unreported = r#"{"warm":false,"caching_observed":false,"hit_ratio":0}"#;
        assert!(collect("deepseek-flash", unreported).is_none());
        assert!(collect("deepseek-flash", "null").is_none());
    }
}
