use chrono::{DateTime, Datelike, FixedOffset, Timelike, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Third-party models with known context windows and prices, see the file for sources.
const BUILTIN_MODELS: &str = include_str!("builtin_models.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(rename = "models", default)]
    pub model_entries: Vec<ModelEntry>,
    #[serde(default)]
    pub context_modifiers: Vec<ContextModifier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub pattern: String,
    #[serde(rename = "match", default)]
    pub match_mode: MatchMode,
    pub display_name: Option<String>,
    /// The model's real context window
    pub context_limit: Option<u32>,
    pub pricing: Option<Pricing>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// The pattern appears anywhere in the model ID
    #[default]
    Contains,
    /// The pattern equals the model ID, ignoring case and trailing tags such as `[1m]`
    Exact,
}

impl ModelEntry {
    fn matches(&self, model_id: &str) -> bool {
        let pattern = self.pattern.to_lowercase();
        match self.match_mode {
            MatchMode::Contains => model_id.to_lowercase().contains(&pattern),
            MatchMode::Exact => {
                strip_bracket_tags(model_id).to_lowercase() == strip_bracket_tags(&pattern)
            }
        }
    }

    /// Like `matches`, but ignoring tags such as `[1m]` in the pattern too
    fn matches_untagged(&self, model_id: &str) -> bool {
        let model_id = strip_bracket_tags(model_id).to_lowercase();
        let pattern = self.pattern.to_lowercase();
        let pattern = strip_bracket_tags(&pattern);
        match self.match_mode {
            MatchMode::Contains => model_id.contains(pattern),
            MatchMode::Exact => model_id == pattern,
        }
    }
}

/// A context window declaration inside model IDs. For example, Claude Code reads
/// `[1m]` as a 1M context window regardless of the base model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextModifier {
    pub pattern: String,
    /// Appended to the display name; empty unless configured
    #[serde(default)]
    pub display_suffix: String,
    pub context_limit: u32,
}

/// Token prices for a model, per 1M tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(flatten)]
    pub rates: Rates,
    /// Rates for larger requests, chosen by the request's input tokens
    #[serde(default)]
    pub tiers: Vec<PricingTier>,
    pub off_peak: Option<OffPeak>,
}

fn default_currency() -> String {
    "$".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    /// Defaults to `input`
    pub cache_read: Option<f64>,
    /// 5-minute cache writes; defaults to `input`
    pub cache_write: Option<f64>,
    /// 1-hour cache writes; defaults to `cache_write`
    pub cache_write_1h: Option<f64>,
}

/// Cache prices a tier leaves out are the base ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingTier {
    /// Applies when the request's input tokens (including cache reads and writes) reach this
    pub min_input: u64,
    #[serde(flatten)]
    pub rates: Rates,
}

/// Discounted hours, as billed by DeepSeek.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffPeak {
    /// Price multiplier outside the peak windows
    pub multiplier: f64,
    /// Hours east of UTC that `peak_hours` are written in, such as 8 or 5.5
    #[serde(default)]
    pub utc_offset: f64,
    /// Full-price windows such as "09:00-12:00", or "22:00-02:00" across midnight
    pub peak_hours: Vec<String>,
    /// Peak windows apply Monday to Friday only
    #[serde(default)]
    pub weekdays_only: bool,
}

/// Token counts of one API response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
}

impl TokenUsage {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Input tokens of the request, which select the pricing tier
    fn request_input(&self) -> u64 {
        self.input + self.cache_read + self.cache_write_5m + self.cache_write_1h
    }
}

impl Pricing {
    /// Cost of one API response; `at` selects off-peak pricing.
    pub fn cost(&self, usage: &TokenUsage, at: Option<DateTime<Utc>>) -> f64 {
        let request_input = usage.request_input();
        let rates = self
            .tiers
            .iter()
            .filter(|tier| request_input >= tier.min_input)
            .max_by_key(|tier| tier.min_input)
            .map_or(&self.rates, |tier| &tier.rates);

        let cache_read = rates
            .cache_read
            .or(self.rates.cache_read)
            .unwrap_or(rates.input);
        let cache_write = rates
            .cache_write
            .or(self.rates.cache_write)
            .unwrap_or(rates.input);
        let cache_write_1h = rates
            .cache_write_1h
            .or(self.rates.cache_write_1h)
            .unwrap_or(cache_write);
        let cost = (usage.input as f64 * rates.input
            + usage.output as f64 * rates.output
            + usage.cache_read as f64 * cache_read
            + usage.cache_write_5m as f64 * cache_write
            + usage.cache_write_1h as f64 * cache_write_1h)
            / 1_000_000.0;

        match (&self.off_peak, at) {
            (Some(off_peak), Some(at)) if !off_peak.is_peak(at) => cost * off_peak.multiplier,
            _ => cost,
        }
    }
}

impl OffPeak {
    fn is_peak(&self, at: DateTime<Utc>) -> bool {
        let Some(offset) = FixedOffset::east_opt((self.utc_offset * 3600.0).round() as i32) else {
            return true;
        };
        let local = at.with_timezone(&offset);
        if self.weekdays_only && local.weekday().number_from_monday() > 5 {
            return false;
        }
        let minute = local.hour() * 60 + local.minute();
        self.peak_hours
            .iter()
            .filter_map(|window| parse_window(window))
            .any(|(start, end)| {
                if start <= end {
                    minute >= start && minute < end
                } else {
                    minute >= start || minute < end
                }
            })
    }
}

/// Parse "HH:MM-HH:MM" into minutes since midnight.
fn parse_window(window: &str) -> Option<(u32, u32)> {
    let minutes = |time: &str| {
        let (hour, minute) = time.trim().split_once(':')?;
        let (hour, minute) = (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?);
        let minutes = hour * 60 + minute;
        (minute < 60 && minutes <= 24 * 60).then_some(minutes)
    };
    let (start, end) = window.split_once('-')?;
    Some((minutes(start)?, minutes(end)?))
}

/// Remove trailing tags such as `[1m]`, which Claude Code uses as context declarations
/// and strips before calling the API.
pub fn strip_bracket_tags(model_id: &str) -> &str {
    let mut id = model_id.trim_end();
    while id.ends_with(']') {
        match id.rfind('[') {
            Some(start) => id = id[..start].trim_end(),
            None => break,
        }
    }
    id
}

/// Built-in Claude model family definition (internal, not serialized).
/// Uses regex with named capture groups to auto-extract version numbers from model IDs.
///
/// Handles both naming conventions:
///   - `claude-{variant}-{major}[-{minor}]-{date}` (e.g., `claude-opus-4-6-20250901`)
///   - `claude-{major}[-{minor}]-{variant}-{date}` (e.g., `claude-4-opus-20250514`)
struct BuiltinModelFamily {
    regex: Regex,
    display_prefix: String,
    context_limit: u32,
}

impl BuiltinModelFamily {
    /// Create a new model family with auto-generated regex pattern.
    ///
    /// The regex matches version numbers (1-2 digits) adjacent to the keyword,
    /// followed by a boundary that signals "version numbers have ended":
    ///   - `-\d{3,}` : date suffix (e.g., `-20250514`)
    ///   - `-[a-z]`  : text qualifier (e.g., `-thinking`, `-preview`, `-latest`)
    ///   - `\[`      : context modifier (e.g., `[1m]`)
    ///   - `$`       : end of string
    ///
    /// The boundary is consumed but only named capture groups are used for version extraction.
    /// Rust's `regex` crate does not support lookahead, so the NFA engine's natural
    /// backtracking prevents date digits from being captured as minor version numbers.
    fn new(keyword: &str, display_prefix: &str, context_limit: u32) -> Self {
        let pattern = format!(
            r"(?:(?P<pre_major>\d{{1,2}})(?:-(?P<pre_minor>\d{{1,2}}))?-{kw}|{kw}-(?P<post_major>\d{{1,2}})(?:-(?P<post_minor>\d{{1,2}}))?)(?:-\d{{3,}}|-[a-z]|\[|$)",
            kw = keyword
        );
        Self {
            regex: Regex::new(&pattern).expect("built-in family regex should compile"),
            display_prefix: display_prefix.to_string(),
            context_limit,
        }
    }

    /// Try to match a model ID (already lowercased) and extract a formatted display name.
    /// Returns `None` if the model ID doesn't match this family.
    fn match_model(&self, model_id_lower: &str) -> Option<String> {
        let caps = self.regex.captures(model_id_lower)?;

        let major = caps
            .name("post_major")
            .or_else(|| caps.name("pre_major"))
            .map(|m| m.as_str())?;

        let minor = caps
            .name("post_minor")
            .or_else(|| caps.name("pre_minor"))
            .map(|m| m.as_str());

        let version = match minor {
            Some(m) => format!("{}.{}", major, m),
            None => major.to_string(),
        };

        Some(format!("{} {}", self.display_prefix, version))
    }
}

/// Lazily-initialized built-in Claude model families.
/// Regex compilation happens only once per process, regardless of how many times
/// `get_display_name()` or `get_inferred_context_limit()` are called.
static BUILTIN_FAMILIES: OnceLock<Vec<BuiltinModelFamily>> = OnceLock::new();

impl ModelConfig {
    /// Get built-in Claude model families (compiled once, cached via OnceLock).
    fn builtin_families() -> &'static [BuiltinModelFamily] {
        BUILTIN_FAMILIES.get_or_init(|| {
            vec![
                BuiltinModelFamily::new("sonnet", "Sonnet", 200_000),
                BuiltinModelFamily::new("opus", "Opus", 200_000),
                BuiltinModelFamily::new("haiku", "Haiku", 200_000),
            ]
        })
    }

    /// Try to match a model ID against built-in Claude model families.
    /// Returns `(display_name, context_limit)` if matched.
    fn match_builtin_family(model_id: &str) -> Option<(String, u32)> {
        let model_lower = model_id.to_lowercase();
        for family in Self::builtin_families() {
            if let Some(name) = family.match_model(&model_lower) {
                return Some((name, family.context_limit));
            }
        }
        None
    }

    /// Load model configuration from TOML file
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let config: ModelConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// The built-in table with the user's models.toml in front. Loaded once per
    /// process, since several segments use it in each render.
    pub fn load() -> &'static Self {
        static MODEL_CONFIG: OnceLock<ModelConfig> = OnceLock::new();
        MODEL_CONFIG.get_or_init(Self::read)
    }

    /// Load model configuration with fallback locations
    fn read() -> Self {
        let mut model_config = Self::default();

        // First, try to create default models.toml if it doesn't exist
        if let Some(home_dir) = dirs::home_dir() {
            let user_models_path = home_dir.join(".claude").join("ccline").join("models.toml");
            if !user_models_path.exists() {
                let _ = Self::create_default_file(&user_models_path);
            }
        }

        if let Some(config) = Self::config_paths()
            .iter()
            .filter(|path| path.exists())
            .find_map(|path| Self::load_from_file(path).ok())
        {
            model_config.merge(config);
        }

        model_config
    }

    /// Where `load` looks for models.toml: the user config directory first, then local
    fn config_paths() -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = dirs::home_dir()
            .map(|d| d.join(".claude").join("ccline").join("models.toml"))
            .into_iter()
            .collect();
        paths.push(PathBuf::from("models.toml"));
        paths
    }

    /// Check the first models.toml that `load` finds. `load` skips a file that does
    /// not parse and reports nothing.
    pub fn check() -> Result<(), Box<dyn std::error::Error>> {
        let Some(path) = Self::config_paths().into_iter().find(|path| path.exists()) else {
            return Ok(());
        };
        let content = fs::read_to_string(&path)?;
        let config: ModelConfig = toml::from_str(&content).map_err(|e| match e.span() {
            Some(span) => {
                let line = content.as_bytes()[..span.start.min(content.len())]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count()
                    + 1;
                format!("line {}: {}", line, e.message())
            }
            None => e.message().to_string(),
        })?;
        for entry in &config.model_entries {
            let off_peak = entry.pricing.as_ref().and_then(|p| p.off_peak.as_ref());
            for window in off_peak.iter().flat_map(|o| &o.peak_hours) {
                if parse_window(window).is_none() {
                    return Err(format!(
                        "{}: peak hours \"{}\" are not HH:MM-HH:MM",
                        entry.pattern, window
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    /// Add a user configuration in front of this one.
    fn merge(&mut self, user: ModelConfig) {
        // Prepend external models to built-in ones for priority
        let mut merged_entries = user.model_entries;
        merged_entries.append(&mut self.model_entries);
        self.model_entries = merged_entries;

        // Prepend external modifiers to built-in ones for priority
        let mut merged_modifiers = user.context_modifiers;
        merged_modifiers.append(&mut self.context_modifiers);
        self.context_modifiers = merged_modifiers;
    }

    /// First value of `field` among entries matching the model, in priority order.
    /// Each field is resolved independently, so a user entry that only renames a
    /// model still inherits the built-in context window and pricing.
    fn entry_field<'a, T>(
        &'a self,
        model_id: &str,
        field: impl Fn(&'a ModelEntry) -> Option<T>,
    ) -> Option<T> {
        self.model_entries
            .iter()
            .filter(|entry| entry.matches(model_id))
            .find_map(field)
    }

    fn find_modifier(&self, model_id: &str) -> Option<&ContextModifier> {
        let model_lower = model_id.to_lowercase();
        self.context_modifiers
            .iter()
            .find(|m| model_lower.contains(&m.pattern.to_lowercase()))
    }

    /// The model's real context window, from a `[[models]]` entry.
    pub fn get_entry_context_limit(&self, model_id: &str) -> Option<u32> {
        self.entry_field(model_id, |entry| entry.context_limit)
    }

    /// The context window Claude Code infers from the model ID, for versions that
    /// do not report `context_window_size`: a context modifier such as `[1m]`, the
    /// built-in Claude family, or the 200k default.
    pub fn get_inferred_context_limit(&self, model_id: &str) -> u32 {
        self.find_modifier(model_id)
            .map(|m| m.context_limit)
            .or_else(|| Self::match_builtin_family(model_id).map(|(_, limit)| limit))
            .unwrap_or(200_000)
    }

    /// Get display name for a model: a `[[models]]` entry, then the built-in Claude
    /// families, plus any configured modifier suffix.
    /// Returns None if nothing matches (caller should use upstream fallback display_name).
    pub fn get_display_name(&self, model_id: &str) -> Option<String> {
        let name = self
            .entry_field(model_id, |entry| entry.display_name.clone())
            .or_else(|| Self::match_builtin_family(model_id).map(|(name, _)| name))?;
        Some(match self.get_display_suffix(model_id) {
            Some(suffix) => format!("{}{}", name, suffix),
            None => name,
        })
    }

    /// Display suffix configured on a matching context modifier, if any.
    pub fn get_display_suffix(&self, model_id: &str) -> Option<String> {
        self.find_modifier(model_id)
            .map(|m| m.display_suffix.clone())
            .filter(|suffix| !suffix.is_empty())
    }

    /// Prices configured for the model. Tags such as `[1m]` are ignored, as Claude Code
    /// strips them before calling the API, so that the model it reports and the same
    /// model in its transcripts get the same prices.
    pub fn get_pricing(&self, model_id: &str) -> Option<&Pricing> {
        self.model_entries
            .iter()
            .filter(|entry| entry.matches_untagged(model_id))
            .find_map(|entry| entry.pricing.as_ref())
    }

    /// Create default model configuration file with minimal template
    pub fn create_default_file<P: AsRef<Path>>(path: P) -> Result<(), Box<dyn std::error::Error>> {
        // Add comments and examples to the template
        let template_content = "# CCometixLine Model Configuration\n\
             # Display names, context windows and prices for models\n\
             # File location: ~/.claude/ccline/models.toml\n\
             #\n\
             # Claude models are automatically recognized (Sonnet, Opus, Haiku) with\n\
             # version extraction. Common third-party models (DeepSeek, GLM, Kimi, Qwen,\n\
             # MiniMax) are built in with their China-platform prices in CNY. Add entries\n\
             # here to override them or to add other models.\n\
             \n\
             # Each [[models]] entry matches a model ID by substring, or exactly with\n\
             # match = \"exact\". A substring also matches longer IDs: \"glm-5.3\" matches\n\
             # \"glm-5.3-flash\" too, so override a single built-in model with\n\
             # match = \"exact\". Entries here take priority over built-in ones, and each\n\
             # field falls back to the next matching entry when left out.\n\
             #\n\
             # context_limit is the model's real context window. The context segment uses\n\
             # the smaller of it and the window Claude Code works with.\n\
             #\n\
             # pricing makes the cost segment compute the session cost from transcripts\n\
             # instead of using Claude Code's figure, which prices unknown models at\n\
             # Claude Opus rates. Rates are per 1M tokens.\n\
             \n\
             # Example:\n\
             # [[models]]\n\
             # pattern = \"my-model\"\n\
             # display_name = \"My Model\"\n\
             # context_limit = 128000\n\
             #\n\
             # [models.pricing]\n\
             # currency = \"$\"\n\
             # input = 0.3\n\
             # output = 1.2\n\
             # cache_read = 0.03       # defaults to input\n\
             # cache_write = 0.3       # 5-minute cache writes, defaults to input\n\
             # cache_write_1h = 0.6    # defaults to cache_write\n\
             #\n\
             # # Higher rates once a request's input tokens reach min_input. Cache\n\
             # # prices left out here are the ones above.\n\
             # [[models.pricing.tiers]]\n\
             # min_input = 32000\n\
             # input = 0.6\n\
             # output = 2.4\n\
             #\n\
             # # Discount outside peak hours\n\
             # [models.pricing.off_peak]\n\
             # multiplier = 0.5\n\
             # utc_offset = 8          # hours east of UTC that peak_hours are in, such as 5.5\n\
             # peak_hours = [\"09:00-12:00\", \"14:00-18:00\"]\n\
             # weekdays_only = true\n\
             \n\
             # Context modifiers are context declarations inside model IDs, used when\n\
             # Claude Code does not report the context window itself. display_suffix\n\
             # optionally appends text to the display name.\n\
             \n\
             # Example:\n\
             # [[context_modifiers]]\n\
             # pattern = \"[1m]\"\n\
             # display_suffix = \" 1M\"\n\
             # context_limit = 1000000\n"
            .to_string();

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, template_content)?;
        Ok(())
    }
}

impl Default for ModelConfig {
    /// The built-in table
    fn default() -> Self {
        toml::from_str(BUILTIN_MODELS).expect("built-in model table should parse")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn utc(s: &str) -> Option<DateTime<Utc>> {
        Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    #[test]
    fn builtin_table_is_consistent() {
        let config = ModelConfig::default();
        let mut seen = HashSet::new();
        for entry in &config.model_entries {
            assert!(seen.insert(entry.pattern.clone()), "{}", entry.pattern);
            assert_eq!(entry.match_mode, MatchMode::Exact, "{}", entry.pattern);
        }
    }

    #[test]
    fn exact_match_ignores_case_and_tags() {
        let config = ModelConfig::default();
        assert_eq!(
            config.get_entry_context_limit("deepseek-flash[1m]"),
            Some(1_000_000)
        );
        assert_eq!(config.get_entry_context_limit("GLM-5.1"), Some(200_000));
        // A newer model is not priced as an older one
        assert!(config.get_pricing("glm-5.4").is_none());
        assert_eq!(config.get_pricing("glm-5").unwrap().rates.input, 4.0);
    }

    #[test]
    fn user_entry_fields_fall_back_to_builtin() {
        let mut config = ModelConfig::default();
        config.merge(
            toml::from_str(
                r#"
                [[models]]
                pattern = "deepseek"
                display_name = "DeepSeek"
                "#,
            )
            .unwrap(),
        );
        assert_eq!(
            config.get_display_name("deepseek-flash[1m]").as_deref(),
            Some("DeepSeek")
        );
        assert_eq!(
            config.get_entry_context_limit("deepseek-flash"),
            Some(1_000_000)
        );
        assert!(config.get_pricing("deepseek-flash").is_some());
    }

    #[test]
    fn no_display_suffix_by_default() {
        let config = ModelConfig::default();
        assert_eq!(
            config.get_display_name("claude-opus-5-5[1m]").as_deref(),
            Some("Opus 5.5")
        );
        assert_eq!(config.get_display_suffix("deepseek-flash[1m]"), None);
        assert_eq!(
            config.get_inferred_context_limit("deepseek-flash[1m]"),
            1_000_000
        );
        assert_eq!(config.get_inferred_context_limit("deepseek-flash"), 200_000);
    }

    #[test]
    fn strips_bracket_tags() {
        assert_eq!(strip_bracket_tags("deepseek-flash[1m]"), "deepseek-flash");
        assert_eq!(strip_bracket_tags("deepseek-v4-pro[1M]"), "deepseek-v4-pro");
        assert_eq!(strip_bracket_tags("glm-5.3"), "glm-5.3");
    }

    #[test]
    fn deepseek_off_peak_is_half_price() {
        let config = ModelConfig::default();
        let pricing = config.get_pricing("deepseek-flash").unwrap();
        let usage = TokenUsage {
            input: 248,
            output: 2,
            cache_read: 11_904,
            ..Default::default()
        };
        // 2026-10-12 is a Monday; 10:00 Beijing is peak, 13:00 and Sunday are off-peak
        let peak = pricing.cost(&usage, utc("2026-10-12T02:00:00Z"));
        let lunch = pricing.cost(&usage, utc("2026-10-12T05:00:00Z"));
        let sunday = pricing.cost(&usage, utc("2026-10-11T02:00:00Z"));
        let expected_peak = (248.0 * 2.0 + 2.0 * 8.0 + 11_904.0 * 0.04) / 1e6;
        assert!((peak - expected_peak).abs() < 1e-12);
        assert!((lunch - expected_peak / 2.0).abs() < 1e-12);
        assert!((sunday - expected_peak / 2.0).abs() < 1e-12);
    }

    #[test]
    fn tier_selected_by_request_input() {
        let config = ModelConfig::default();
        let pricing = config.get_pricing("qwen3-coder-plus").unwrap();
        let small = TokenUsage {
            input: 1_000,
            cache_read: 20_000,
            output: 100,
            ..Default::default()
        };
        let large = TokenUsage {
            input: 1_000,
            cache_read: 140_000,
            output: 100,
            ..Default::default()
        };
        let small_cost = pricing.cost(&small, None);
        let large_cost = pricing.cost(&large, None);
        assert!((small_cost - (1_000.0 * 4.0 + 20_000.0 * 0.4 + 100.0 * 16.0) / 1e6).abs() < 1e-12);
        assert!(
            (large_cost - (1_000.0 * 10.0 + 140_000.0 * 1.0 + 100.0 * 40.0) / 1e6).abs() < 1e-12
        );
    }

    #[test]
    fn tier_cache_prices_fall_back_to_the_base_ones() {
        // The tier from the models.toml template, which leaves cache prices out
        let pricing: Pricing = toml::from_str(
            r#"
            input = 0.3
            output = 1.2
            cache_read = 0.03

            [[tiers]]
            min_input = 32000
            input = 0.6
            output = 2.4
            "#,
        )
        .unwrap();
        let usage = TokenUsage {
            input: 1_000,
            cache_read: 100_000,
            output: 500,
            ..Default::default()
        };
        let expected = (1_000.0 * 0.6 + 500.0 * 2.4 + 100_000.0 * 0.03) / 1e6;
        assert!((pricing.cost(&usage, None) - expected).abs() < 1e-12);
    }

    #[test]
    fn prices_ignore_tags_in_patterns() {
        let mut config = ModelConfig::default();
        config.merge(
            toml::from_str(
                r#"
                [[models]]
                pattern = "deepseek-v4-pro[1m]"
                [models.pricing]
                currency = "$"
                input = 1.0
                output = 2.0

                [[models]]
                pattern = "glm-5.1[1m]"
                match = "exact"
                display_name = "GLM"
                "#,
            )
            .unwrap(),
        );
        // As Claude Code reports the model, and as its transcripts record it
        assert_eq!(
            config.get_pricing("deepseek-v4-pro[1m]").unwrap().currency,
            "$"
        );
        assert_eq!(config.get_pricing("deepseek-v4-pro").unwrap().currency, "$");
        // Exact patterns ignore tags on both sides
        assert_eq!(config.get_display_name("glm-5.1").as_deref(), Some("GLM"));
        assert_eq!(
            config.get_display_name("glm-5.1[1m]").as_deref(),
            Some("GLM")
        );
    }

    #[test]
    fn peak_hours_take_fractional_offsets_and_cross_midnight() {
        let off_peak: OffPeak = toml::from_str(
            r#"
            multiplier = 0.5
            utc_offset = 5.5
            peak_hours = ["22:00-02:00"]
            "#,
        )
        .unwrap();
        // 23:30 and 01:00 in UTC+5:30 are peak, 03:00 is not
        assert!(off_peak.is_peak(utc("2026-10-12T18:00:00Z").unwrap()));
        assert!(off_peak.is_peak(utc("2026-10-12T19:30:00Z").unwrap()));
        assert!(!off_peak.is_peak(utc("2026-10-12T21:30:00Z").unwrap()));

        assert_eq!(parse_window("09:00-24:00"), Some((540, 1440)));
        assert_eq!(parse_window("09:60-12:00"), None);
        assert_eq!(parse_window("25:00-26:00"), None);
        assert_eq!(parse_window("9-12"), None);
    }

    #[test]
    fn one_hour_cache_writes_use_their_own_rate() {
        let config = ModelConfig::default();
        let pricing = config.get_pricing("kimi-k3").unwrap();
        let usage = TokenUsage {
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
            ..Default::default()
        };
        assert!((pricing.cost(&usage, None) - 60.0).abs() < 1e-9);
    }
}
