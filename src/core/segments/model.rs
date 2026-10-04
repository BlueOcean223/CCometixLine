use super::{Segment, SegmentData};
use crate::config::{strip_bracket_tags, InputData, ModelConfig, SegmentId};
use std::collections::HashMap;

#[derive(Default)]
pub struct ModelSegment;

impl ModelSegment {
    pub fn new() -> Self {
        Self
    }
}

impl Segment for ModelSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let mut metadata = HashMap::new();
        metadata.insert("model_id".to_string(), input.model.id.clone());
        metadata.insert("display_name".to_string(), input.model.display_name.clone());

        Some(SegmentData {
            primary: self.format_model_name(&input.model.id, &input.model.display_name),
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Model
    }
}

impl ModelSegment {
    fn format_model_name(&self, id: &str, display_name: &str) -> String {
        let model_config = ModelConfig::load();

        if let Some(config_name) = model_config.get_display_name(id) {
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
            match model_config.get_display_suffix(id) {
                Some(suffix) => format!("{}{}", base, suffix),
                None => base.to_string(),
            }
        }
    }
}
