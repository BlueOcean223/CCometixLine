use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Main config structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub style: StyleConfig,
    pub segments: Vec<SegmentConfig>,
    pub theme: String,
}

// Default implementation moved to ui/themes/presets.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleConfig {
    pub mode: StyleMode,
    pub separator: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleMode {
    Plain,
    NerdFont,
    Powerline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentConfig {
    pub id: SegmentId,
    pub enabled: bool,
    pub icon: IconConfig,
    pub colors: ColorConfig,
    pub styles: TextStyleConfig,
    pub options: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IconConfig {
    pub plain: String,
    pub nerd_font: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorConfig {
    pub icon: Option<AnsiColor>,
    pub text: Option<AnsiColor>,
    pub background: Option<AnsiColor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextStyleConfig {
    pub text_bold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnsiColor {
    Color16 { c16: u8 },
    Color256 { c256: u8 },
    Rgb { r: u8, g: u8, b: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentId {
    Model,
    Directory,
    Git,
    ContextWindow,
    Usage,
    Cost,
    Session,
    OutputStyle,
    Update,
    PromptCache,
}

impl SegmentId {
    /// Name shown in the configuration UI
    pub fn name(&self) -> &'static str {
        match self {
            SegmentId::Model => "Model",
            SegmentId::Directory => "Directory",
            SegmentId::Git => "Git",
            SegmentId::ContextWindow => "Context Window",
            SegmentId::Usage => "Usage",
            SegmentId::Cost => "Cost",
            SegmentId::Session => "Session",
            SegmentId::OutputStyle => "Output Style",
            SegmentId::Update => "Update",
            SegmentId::PromptCache => "Prompt Cache",
        }
    }

    /// Options the segment reads from `[segments.options]`, in the order the
    /// configuration UI lists them
    pub fn options(&self) -> &'static [SegmentOption] {
        match self {
            SegmentId::Model => MODEL_OPTIONS,
            SegmentId::Git => GIT_OPTIONS,
            SegmentId::Usage => USAGE_OPTIONS,
            SegmentId::PromptCache => PROMPT_CACHE_OPTIONS,
            _ => &[],
        }
    }
}

const MODEL_OPTIONS: &[SegmentOption] = &[
    SegmentOption::new("show_effort", "Effort Level", OptionDefault::Toggle(true)),
    SegmentOption::new("show_fast_mode", "Fast Mode", OptionDefault::Toggle(true)),
];

const GIT_OPTIONS: &[SegmentOption] = &[SegmentOption::new(
    "show_sha",
    "Commit SHA",
    OptionDefault::Toggle(false),
)];

const USAGE_OPTIONS: &[SegmentOption] = &[
    SegmentOption::new("show_reset_time", "Reset Time", OptionDefault::Toggle(true)),
    SegmentOption::new("show_seven_day", "7-Day Usage", OptionDefault::Toggle(true)),
    // For the usage API, queried only when Claude Code does not report rate limits
    SegmentOption::new(
        "api_base_url",
        "API Base URL",
        OptionDefault::Text("https://api.anthropic.com"),
    ),
    SegmentOption::new(
        "cache_duration",
        "API Cache Duration",
        OptionDefault::Seconds(180),
    ),
    SegmentOption::new("timeout", "API Timeout", OptionDefault::Seconds(2)),
];

const PROMPT_CACHE_OPTIONS: &[SegmentOption] = &[SegmentOption::new(
    "show_expiry",
    "Expiry Time",
    OptionDefault::Toggle(true),
)];

/// An entry in a segment's `[segments.options]`
pub struct SegmentOption {
    pub key: &'static str,
    /// Name shown in the configuration UI
    pub label: &'static str,
    /// The option's type, and its value when the config leaves it out
    pub default: OptionDefault,
}

impl SegmentOption {
    const fn new(key: &'static str, label: &'static str, default: OptionDefault) -> Self {
        Self {
            key,
            label,
            default,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OptionDefault {
    Toggle(bool),
    Text(&'static str),
    /// A whole number of seconds
    Seconds(u64),
}

impl OptionDefault {
    /// Whether a value from the config has this option's type
    pub fn accepts(&self, value: &serde_json::Value) -> bool {
        match self {
            OptionDefault::Toggle(_) => value.is_boolean(),
            OptionDefault::Text(_) => value.is_string(),
            OptionDefault::Seconds(_) => value.is_u64(),
        }
    }
}

impl From<OptionDefault> for serde_json::Value {
    fn from(default: OptionDefault) -> Self {
        match default {
            OptionDefault::Toggle(on) => on.into(),
            OptionDefault::Text(text) => text.into(),
            OptionDefault::Seconds(seconds) => seconds.into(),
        }
    }
}

impl SegmentConfig {
    /// One of the segment's `SegmentId::options`: the config's value, or the
    /// default when the config leaves it out or gives a value of another type
    pub fn option(&self, key: &str) -> serde_json::Value {
        let Some(option) = self.id.options().iter().find(|option| option.key == key) else {
            return serde_json::Value::Null;
        };
        match self.options.get(key) {
            Some(value) if option.default.accepts(value) => value.clone(),
            _ => option.default.into(),
        }
    }

    /// Whether one of the segment's toggles is on
    pub fn toggle(&self, key: &str) -> bool {
        self.option(key).as_bool().unwrap_or(false)
    }
}

// Legacy compatibility structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SegmentsConfig {
    pub directory: bool,
    pub git: bool,
    pub model: bool,
    // pub usage: bool,
}

// Data structures compatible with existing main.rs
#[derive(Deserialize)]
pub struct Model {
    pub id: String,
    pub display_name: String,
}

#[derive(Deserialize)]
pub struct Workspace {
    pub current_dir: String,
    /// Where Claude Code was started, whose `.claude` directory holds the project's settings
    #[serde(default, deserialize_with = "lenient")]
    pub project_dir: Option<String>,
}

#[derive(Deserialize)]
pub struct Cost {
    pub total_cost_usd: Option<f64>,
    pub total_duration_ms: Option<u64>,
    pub total_api_duration_ms: Option<u64>,
    pub total_lines_added: Option<u32>,
    pub total_lines_removed: Option<u32>,
}

#[derive(Deserialize)]
pub struct OutputStyle {
    pub name: String,
}

#[derive(Deserialize)]
pub struct ContextWindow {
    pub context_window_size: Option<u32>,
    /// `null` before the first API response and right after `/compact`
    pub current_usage: Option<CurrentUsage>,
}

#[derive(Deserialize)]
pub struct CurrentUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

impl CurrentUsage {
    /// Tokens occupying the context window, including the last response's output,
    /// which becomes input on the next request. Same formula as the transcript
    /// fallback (`NormalizedUsage::context_tokens`); Claude Code's own
    /// `used_percentage` excludes output tokens.
    pub fn context_tokens(&self) -> u32 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }
}

/// Present only for claude.ai Pro/Max subscribers, after the first API response.
/// Each window may be independently absent; Claude Code drops a window once it resets.
#[derive(Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<RateLimitWindow>,
    pub seven_day: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
pub struct RateLimitWindow {
    /// 0 to 100
    pub used_percentage: f64,
    /// Unix epoch seconds
    pub resets_at: Option<i64>,
}

/// Absent when the model does not support the effort parameter
#[derive(Deserialize)]
pub struct Effort {
    /// `low`, `medium`, `high`, `xhigh` or `max`
    pub level: String,
}

/// Prompt cache statistics of the main conversation, present after its first API response
#[derive(Deserialize)]
pub struct PromptCache {
    /// Whether the cached prefix is still within its lifetime
    #[serde(default)]
    pub warm: bool,
    /// Whether any response this session reported cache tokens. `false` when caching
    /// is off or the provider does not report it
    #[serde(default)]
    pub caching_observed: bool,
    /// Unix epoch seconds when the cached prefix goes cold
    pub expires_at: Option<i64>,
    /// Cache reads as a fraction of all input tokens this session, 0 to 1
    pub hit_ratio: Option<f64>,
}

#[derive(Deserialize)]
pub struct InputData {
    pub model: Model,
    pub workspace: Workspace,
    pub transcript_path: String,
    pub cost: Option<Cost>,
    pub output_style: Option<OutputStyle>,
    /// Claude Code version
    pub version: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub context_window: Option<ContextWindow>,
    #[serde(default, deserialize_with = "lenient")]
    pub rate_limits: Option<RateLimits>,
    #[serde(default, deserialize_with = "lenient")]
    pub effort: Option<Effort>,
    #[serde(default, deserialize_with = "lenient")]
    pub fast_mode: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    pub prompt_cache: Option<PromptCache>,
}

/// Deserialize an optional field, treating a malformed value as absent so that
/// a shape change in a newer Claude Code field does not blank the whole statusline.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

// OpenAI-style nested token details
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
    #[serde(default)]
    pub audio_tokens: Option<u32>,
}

// Raw usage data from different LLM providers (flexible parsing)
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RawUsage {
    // Anthropic-style input tokens
    #[serde(default)]
    pub input_tokens: Option<u32>,

    // OpenAI-style input tokens (separate field to handle both formats)
    #[serde(default)]
    pub prompt_tokens: Option<u32>,

    // Anthropic-style output tokens
    #[serde(default)]
    pub output_tokens: Option<u32>,

    // OpenAI-style output tokens (separate field to handle both formats)
    #[serde(default)]
    pub completion_tokens: Option<u32>,

    // Total tokens (some providers only provide this)
    #[serde(default)]
    pub total_tokens: Option<u32>,

    // Anthropic-style cache fields
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,

    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,

    // OpenAI-style cache fields (separate fields to handle both formats)
    #[serde(default)]
    pub cache_creation_prompt_tokens: Option<u32>,

    #[serde(default)]
    pub cache_read_prompt_tokens: Option<u32>,

    #[serde(default)]
    pub cached_tokens: Option<u32>,

    // OpenAI-style nested details
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,

    // Completion token details (OpenAI)
    #[serde(default)]
    pub completion_tokens_details: Option<HashMap<String, u32>>,

    // Catch unknown fields for future compatibility and debugging
    #[serde(flatten, skip_serializing)]
    pub extra: HashMap<String, serde_json::Value>,
}

// Normalized internal representation after processing
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct NormalizedUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,

    // Metadata for debugging and analysis
    pub calculation_source: String,
    pub raw_data_available: Vec<String>,
}

impl NormalizedUsage {
    /// Get tokens that count toward context window
    /// This includes all tokens that consume context window space
    /// Output tokens from this turn will become input tokens in the next turn
    pub fn context_tokens(&self) -> u32 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }

    /// Get total tokens for cost calculation
    /// Priority: use total_tokens if available, otherwise sum all components
    pub fn total_for_cost(&self) -> u32 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.input_tokens
                + self.output_tokens
                + self.cache_creation_input_tokens
                + self.cache_read_input_tokens
        }
    }

    /// Get the most appropriate token count for general display
    /// For OpenAI format: use total_tokens directly
    /// For Anthropic format: use context_tokens (input + cache)
    pub fn display_tokens(&self) -> u32 {
        // For Claude/Anthropic format: prefer input-related tokens for context window display
        let context = self.context_tokens();
        if context > 0 {
            return context;
        }

        // For OpenAI format: use total_tokens when no input breakdown available
        if self.total_tokens > 0 {
            return self.total_tokens;
        }

        // Fallback to any available tokens
        self.input_tokens.max(self.output_tokens)
    }
}

impl Config {
    /// Check if current config matches the specified theme preset
    pub fn matches_theme(&self, theme_name: &str) -> bool {
        let theme_preset = crate::ui::themes::ThemePresets::get_theme(theme_name);

        // Compare style config
        if self.style.mode != theme_preset.style.mode
            || self.style.separator != theme_preset.style.separator
        {
            return false;
        }

        // Compare segments count and order
        if self.segments.len() != theme_preset.segments.len() {
            return false;
        }

        // Compare each segment config
        for (current, preset) in self.segments.iter().zip(theme_preset.segments.iter()) {
            if !self.segment_matches(current, preset) {
                return false;
            }
        }

        true
    }

    /// Add segments that the config lacks because an older version wrote it, so
    /// that they can be enabled in the configuration UI. Each takes its place and
    /// settings from the built-in theme and starts disabled.
    pub fn add_missing_segments(&mut self) {
        let preset = crate::ui::themes::ThemePresets::builtin_theme(&self.theme);
        for (index, segment) in preset.segments.iter().enumerate() {
            if self.segments.iter().any(|s| s.id == segment.id) {
                continue;
            }
            // After the nearest segment that precedes it in the theme
            let position = preset.segments[..index]
                .iter()
                .rev()
                .find_map(|prev| self.segments.iter().position(|s| s.id == prev.id))
                .map_or(0, |i| i + 1);
            let mut segment = segment.clone();
            segment.enabled = false;
            self.segments.insert(position, segment);
        }
    }

    /// Check if current config has been modified from the selected theme
    pub fn is_modified_from_theme(&self) -> bool {
        !self.matches_theme(&self.theme)
    }

    /// Compare two segment configs for equality
    fn segment_matches(&self, current: &SegmentConfig, preset: &SegmentConfig) -> bool {
        current.id == preset.id
            && current.enabled == preset.enabled
            && current.icon.plain == preset.icon.plain
            && current.icon.nerd_font == preset.icon.nerd_font
            && self.color_matches(&current.colors.icon, &preset.colors.icon)
            && self.color_matches(&current.colors.text, &preset.colors.text)
            && self.color_matches(&current.colors.background, &preset.colors.background)
            && current.styles.text_bold == preset.styles.text_bold
            && current
                .id
                .options()
                .iter()
                .all(|option| current.option(option.key) == preset.option(option.key))
    }

    /// Compare two optional colors for equality
    fn color_matches(&self, current: &Option<AnsiColor>, preset: &Option<AnsiColor>) -> bool {
        match (current, preset) {
            (None, None) => true,
            (Some(c1), Some(c2)) => c1 == c2,
            _ => false,
        }
    }
}

impl PartialEq for AnsiColor {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AnsiColor::Color16 { c16: a }, AnsiColor::Color16 { c16: b }) => a == b,
            (AnsiColor::Color256 { c256: a }, AnsiColor::Color256 { c256: b }) => a == b,
            (
                AnsiColor::Rgb {
                    r: r1,
                    g: g1,
                    b: b1,
                },
                AnsiColor::Rgb {
                    r: r2,
                    g: g2,
                    b: b2,
                },
            ) => r1 == r2 && g1 == g2 && b1 == b2,
            _ => false,
        }
    }
}

impl RawUsage {
    /// Convert raw usage data to normalized format with intelligent token inference
    pub fn normalize(self) -> NormalizedUsage {
        let mut result = NormalizedUsage::default();
        let mut sources = Vec::new();

        // Collect available raw data fields and merge tokens with Anthropic priority
        let mut available_fields = Vec::new();

        // Merge input tokens (priority: input_tokens > prompt_tokens)
        let input = self.input_tokens.or(self.prompt_tokens).unwrap_or(0);
        if input > 0 {
            available_fields.push("input_tokens".to_string());
        }

        // Merge output tokens (priority: output_tokens > completion_tokens)
        let output = self.output_tokens.or(self.completion_tokens).unwrap_or(0);
        if output > 0 {
            available_fields.push("output_tokens".to_string());
        }

        let total = self.total_tokens.unwrap_or(0);
        if total > 0 {
            available_fields.push("total_tokens".to_string());
        }

        // Merge cache creation tokens (priority: Anthropic > OpenAI)
        let cache_creation = self
            .cache_creation_input_tokens
            .or(self.cache_creation_prompt_tokens)
            .unwrap_or(0);
        if cache_creation > 0 {
            available_fields.push("cache_creation".to_string());
        }

        // Merge cache read tokens (priority: Anthropic > OpenAI > nested format)
        let cache_read = self
            .cache_read_input_tokens
            .or(self.cache_read_prompt_tokens)
            .or(self.cached_tokens)
            .or_else(|| {
                // Fallback to OpenAI nested format
                self.prompt_tokens_details
                    .as_ref()
                    .and_then(|d| d.cached_tokens)
            })
            .unwrap_or(0);
        if cache_read > 0 {
            available_fields.push("cache_read".to_string());
        }

        result.raw_data_available = available_fields;

        // Use merged cache values (already calculated above with Anthropic priority)

        // Token calculation logic - prioritize total_tokens for OpenAI format
        let total_value = if total > 0 {
            sources.push("total_tokens_direct".to_string());
            total
        } else if input > 0 || output > 0 || cache_read > 0 || cache_creation > 0 {
            let calculated = input + output + cache_read + cache_creation;
            sources.push("total_from_components".to_string());
            calculated
        } else {
            0
        };

        // Assignment
        result.input_tokens = input;
        result.output_tokens = output;
        result.total_tokens = total_value;
        result.cache_creation_input_tokens = cache_creation;
        result.cache_read_input_tokens = cache_read;
        result.calculation_source = sources.join("+");

        result
    }
}

// Legacy alias for backward compatibility
pub type Usage = RawUsage;

#[derive(Deserialize)]
pub struct Message {
    pub usage: Option<Usage>,
}

#[derive(Deserialize)]
pub struct TranscriptEntry {
    pub r#type: Option<String>,
    /// For `system` entries, such as `compact_boundary` written by `/compact`
    pub subtype: Option<String>,
    pub message: Option<Message>,
    #[serde(rename = "leafUuid")]
    pub leaf_uuid: Option<String>,
    pub uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    pub parent_uuid: Option<String>,
    pub summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::ThemePresets;

    #[test]
    fn missing_segments_are_added_disabled_in_theme_order() {
        let preset = ThemePresets::builtin_theme("nord");
        // A config written before the prompt cache segment existed
        let mut config = preset.clone();
        config
            .segments
            .retain(|segment| segment.id != SegmentId::PromptCache);
        for segment in &mut config.segments {
            segment.enabled = true;
        }

        config.add_missing_segments();
        let ids: Vec<_> = config.segments.iter().map(|s| s.id).collect();
        let preset_ids: Vec<_> = preset.segments.iter().map(|s| s.id).collect();
        assert_eq!(ids, preset_ids);
        let added = config
            .segments
            .iter()
            .find(|s| s.id == SegmentId::PromptCache)
            .unwrap();
        assert!(!added.enabled);

        // Nothing changes once every segment is present
        let before = config.segments.len();
        config.add_missing_segments();
        assert_eq!(config.segments.len(), before);
    }

    #[test]
    fn options_fall_back_to_their_defaults() {
        let preset = ThemePresets::builtin_theme("default");
        let mut model = preset.segments[0].clone();
        assert_eq!(model.id, SegmentId::Model);
        model.options.clear();
        assert!(model.toggle("show_effort"));

        model
            .options
            .insert("show_effort".to_string(), serde_json::Value::Bool(false));
        assert!(!model.toggle("show_effort"));
        // A value of the wrong type counts as left out
        model
            .options
            .insert("show_fast_mode".to_string(), serde_json::json!("no"));
        assert!(model.toggle("show_fast_mode"));
        // Keys outside the segment's options have no value
        assert_eq!(model.option("show_sha"), serde_json::Value::Null);

        let mut usage = model.clone();
        usage.id = SegmentId::Usage;
        assert_eq!(usage.option("timeout"), serde_json::json!(2));
        assert_eq!(
            usage.option("api_base_url"),
            serde_json::json!("https://api.anthropic.com")
        );
    }

    #[test]
    fn theme_options_have_the_types_segments_read() {
        for (name, _) in ThemePresets::get_available_themes() {
            for segment in ThemePresets::builtin_theme(name).segments {
                for (key, value) in &segment.options {
                    let option = segment.id.options().iter().find(|o| o.key == key);
                    assert!(
                        option.is_some_and(|o| o.default.accepts(value)),
                        "{} theme, {:?} segment: {} = {}",
                        name,
                        segment.id,
                        key,
                        value
                    );
                }
            }
        }
    }

    #[test]
    fn options_set_to_their_defaults_still_match_the_theme() {
        let config = ThemePresets::builtin_theme("default");
        let preset = &config.segments[0];
        let mut model = preset.clone();
        model
            .options
            .insert("show_effort".to_string(), serde_json::Value::Bool(true));
        assert!(config.segment_matches(&model, preset));
        model
            .options
            .insert("show_effort".to_string(), serde_json::Value::Bool(false));
        assert!(!config.segment_matches(&model, preset));
    }
}
