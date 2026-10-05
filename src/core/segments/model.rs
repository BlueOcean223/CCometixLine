use super::{join_details, Segment, SegmentData};
use crate::config::{strip_bracket_tags, InputData, ModelConfig, SegmentId};
use std::collections::HashMap;

pub struct ModelSegment {
    show_effort: bool,
    show_fast_mode: bool,
}

impl Default for ModelSegment {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelSegment {
    pub fn new() -> Self {
        Self {
            show_effort: true,
            show_fast_mode: true,
        }
    }

    pub fn with_effort(mut self, show_effort: bool) -> Self {
        self.show_effort = show_effort;
        self
    }

    pub fn with_fast_mode(mut self, show_fast_mode: bool) -> Self {
        self.show_fast_mode = show_fast_mode;
        self
    }
}

impl Segment for ModelSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        Some(self.segment_data(input, &ModelConfig::load()))
    }

    fn id(&self) -> SegmentId {
        SegmentId::Model
    }
}

impl ModelSegment {
    fn segment_data(&self, input: &InputData, models: &ModelConfig) -> SegmentData {
        let mut metadata = HashMap::new();
        metadata.insert("model_id".to_string(), input.model.id.clone());
        metadata.insert("display_name".to_string(), input.model.display_name.clone());

        // Secondary display: reasoning effort and fast mode
        let mut details = Vec::new();
        if let Some(effort) = &input.effort {
            metadata.insert("effort".to_string(), effort.level.clone());
            if self.show_effort {
                details.push(effort.level.clone());
            }
        }
        if input.fast_mode == Some(true) {
            metadata.insert("fast_mode".to_string(), "true".to_string());
            if self.show_fast_mode {
                details.push("fast".to_string());
            }
        }
        SegmentData {
            primary: Self::format_model_name(models, &input.model.id, &input.model.display_name),
            secondary: join_details(&details),
            metadata,
        }
    }

    fn format_model_name(models: &ModelConfig, id: &str, display_name: &str) -> String {
        if let Some(config_name) = models.get_display_name(id) {
            // Model recognized by config, display_name already includes modifier suffix
            config_name
        } else {
            // Fallback: prefer upstream display_name, fall back to model_id if empty.
            // For models it does not know, Claude Code passes the raw ID including
            // context declarations such as `[1m]`, which are not part of the name.
            let base = strip_bracket_tags(if display_name.is_empty() {
                id
            } else {
                display_name
            });
            // Apply a configured context modifier suffix, if any
            match models.get_display_suffix(id) {
                Some(suffix) => format!("{}{}", base, suffix),
                None => base.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(extra: &str) -> InputData {
        let json = format!(
            r#"{{"model":{{"id":"deepseek-flash[1m]","display_name":"deepseek-flash[1m]"}},"workspace":{{"current_dir":"/tmp"}},"transcript_path":"/tmp/t.jsonl"{}}}"#,
            extra
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn shows_effort_and_fast_mode() {
        let input = input(r#","effort":{"level":"max"},"fast_mode":true"#);
        let models = ModelConfig::default();
        let data = ModelSegment::new().segment_data(&input, &models);
        assert_eq!(data.primary, "deepseek-flash");
        assert_eq!(data.secondary, "· max · fast");

        let data = ModelSegment::new()
            .with_effort(false)
            .segment_data(&input, &models);
        assert_eq!(data.secondary, "· fast");

        let data = ModelSegment::new()
            .with_fast_mode(false)
            .segment_data(&input, &models);
        assert_eq!(data.secondary, "· max");
    }

    #[test]
    fn nothing_extra_without_effort_or_fast_mode() {
        let models = ModelConfig::default();
        let data = ModelSegment::new().segment_data(&input(r#","fast_mode":false"#), &models);
        assert_eq!(data.secondary, "");
        // A malformed value does not break the input
        let data = ModelSegment::new().segment_data(&input(r#","effort":{"level":3}"#), &models);
        assert_eq!(data.secondary, "");
    }
}
