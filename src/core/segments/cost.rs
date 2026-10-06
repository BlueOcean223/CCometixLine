use super::{Segment, SegmentData};
use crate::config::{InputData, ModelConfig, SegmentId};
use crate::utils::session_cost::session_cost;
use std::collections::HashMap;
use std::path::Path;

#[derive(Default)]
pub struct CostSegment;

impl CostSegment {
    pub fn new() -> Self {
        Self
    }
}

impl Segment for CostSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        // Claude Code prices models it does not know at Claude Opus rates, so models
        // with configured prices are priced from the session's transcripts instead
        let models = ModelConfig::load();
        let configured = models.get_pricing(&input.model.id).map(|pricing| {
            let cost = session_cost(Path::new(&input.transcript_path), models, &pricing.currency);
            (
                cost.total,
                pricing.currency.clone(),
                "models_toml",
                cost.unpriced,
            )
        });
        let (cost, currency, source, unpriced) = match configured {
            Some(configured) => configured,
            None => (
                input.cost.as_ref()?.total_cost_usd?,
                "$".to_string(),
                "claude_code",
                0,
            ),
        };

        // Primary display: total cost
        let mut primary = if cost < 0.01 {
            format!("{}0", currency)
        } else {
            format!("{}{:.2}", currency, cost)
        };
        // Some responses have no price in the currency, so the session cost more
        if unpriced > 0 {
            primary.push('+');
        }

        // Secondary display: empty for cost segment
        let secondary = String::new();

        let mut metadata = HashMap::new();
        metadata.insert("cost".to_string(), cost.to_string());
        metadata.insert("currency".to_string(), currency);
        metadata.insert("source".to_string(), source.to_string());
        metadata.insert("unpriced_responses".to_string(), unpriced.to_string());

        Some(SegmentData {
            primary,
            secondary,
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Cost
    }
}
