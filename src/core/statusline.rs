use crate::config::{AnsiColor, Config, SegmentConfig, StyleMode};
use crate::core::segments::SegmentData;
use unicode_width::UnicodeWidthStr;

/// Display width of text in terminal columns, skipping escape sequences: CSI
/// (`ESC [ ... final byte`, used for colors) and OSC (`ESC ] ... BEL` or
/// `ESC ] ... ESC \`, used for hyperlinks).
fn visible_width(text: &str) -> usize {
    let mut visible = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            visible.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    visible.width()
}

/// Columns available to the status line. Claude Code passes the terminal width in
/// `COLUMNS` and keeps 4 columns plus the `statusLine.padding` setting on both
/// sides for itself; it cuts longer lines off (measured with Claude Code 2.1.284).
pub fn status_line_width(columns: &str, padding: usize) -> Option<usize> {
    let columns: usize = columns.trim().parse().ok()?;
    columns
        .checked_sub(4 + 2 * padding)
        .filter(|&width| width > 0)
}

/// `statusLine.padding` in the user's Claude Code settings
pub fn status_line_padding() -> usize {
    dirs::home_dir()
        .and_then(|home| std::fs::read_to_string(home.join(".claude").join("settings.json")).ok())
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|settings| settings.get("statusLine")?.get("padding")?.as_u64())
        .map_or(0, |padding| padding as usize)
}

pub struct StatusLineGenerator {
    config: Config,
    max_width: Option<usize>,
}

impl StatusLineGenerator {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            max_width: None,
        }
    }

    /// Wrap lines between segments to fit `max_width` terminal columns
    pub fn with_max_width(mut self, max_width: Option<usize>) -> Self {
        self.max_width = max_width;
        self
    }

    pub fn generate(&self, segments: Vec<(SegmentConfig, SegmentData)>) -> String {
        self.layout(&segments, self.max_width).join("\n")
    }

    /// Render the enabled segments and join them with separators. With a maximum
    /// width, a new line starts before a segment that would make the line wider;
    /// lines only break between segments.
    fn layout(
        &self,
        segments: &[(SegmentConfig, SegmentData)],
        max_width: Option<usize>,
    ) -> Vec<String> {
        let rendered: Vec<(&SegmentConfig, String)> = segments
            .iter()
            .filter(|(config, _)| config.enabled)
            .map(|(config, data)| (config, self.render_segment(config, data)))
            .filter(|(_, text)| !text.is_empty())
            .collect();

        let powerline = self.config.style.separator == "\u{e0b0}";
        let mut lines = Vec::new();
        let mut line = String::new();
        let mut width = 0;
        for (i, (config, text)) in rendered.iter().enumerate() {
            let text_width = visible_width(text);
            if i > 0 {
                let separator = if powerline {
                    // Arrow colored from the previous segment's background to this one's
                    self.create_powerline_arrow(
                        rendered[i - 1].0.colors.background.as_ref(),
                        config.colors.background.as_ref(),
                    )
                } else {
                    format!("\x1b[37m{}\x1b[0m", self.config.style.separator)
                };
                let separator_width = visible_width(&separator);
                if max_width.is_some_and(|max| width + separator_width + text_width > max) {
                    lines.push(std::mem::take(&mut line));
                    width = 0;
                } else {
                    line.push_str(&separator);
                    width += separator_width;
                }
            }
            line.push_str(text);
            width += text_width;
        }
        if !line.is_empty() {
            lines.push(line);
        }
        if powerline {
            for line in &mut lines {
                line.push_str("\x1b[0m");
            }
        }
        lines
    }

    /// Generate statusline for TUI preview with proper width calculation
    /// This method handles ANSI escape sequences properly for ratatui rendering
    pub fn generate_for_tui(
        &self,
        segments: Vec<(SegmentConfig, SegmentData)>,
    ) -> ratatui::text::Line<'static> {
        use ansi_to_tui::IntoText;
        use ratatui::text::{Line, Span};

        // Use the same generate method and convert to TUI
        let full_output = self.generate(segments);

        if let Ok(text) = full_output.into_text() {
            if let Some(line) = text.lines.into_iter().next() {
                return line;
            }
        }

        // Fallback to raw text
        Line::from(vec![Span::raw(full_output)])
    }

    /// Generate TUI-optimized text with intelligent wrapping by segment for preview
    pub fn generate_for_tui_preview(
        &self,
        segments: Vec<(SegmentConfig, SegmentData)>,
        max_width: u16,
    ) -> ratatui::text::Text<'_> {
        use ansi_to_tui::IntoText;
        use ratatui::text::{Line, Span, Text};

        let lines = self.layout(&segments, Some(max_width as usize));

        // Convert string lines to ratatui Text
        let mut tui_lines = Vec::new();
        for line in lines {
            if let Ok(text) = line.into_text() {
                for tui_line in text.lines {
                    tui_lines.push(tui_line);
                }
            } else {
                tui_lines.push(Line::from(vec![Span::raw(line)]));
            }
        }

        // Ensure we have at least one line
        if tui_lines.is_empty() {
            tui_lines.push(Line::default());
        }

        Text::from(tui_lines)
    }

    fn render_segment(&self, config: &SegmentConfig, data: &SegmentData) -> String {
        let icon = if let Some(dynamic_icon) = data.metadata.get("dynamic_icon") {
            dynamic_icon.clone()
        } else {
            self.get_icon(config)
        };

        // Apply background color to the entire segment if set
        if let Some(bg_color) = &config.colors.background {
            let bg_code = self.apply_background_color(bg_color);

            // Build the entire segment content first
            let icon_colored = if let Some(icon_color) = &config.colors.icon {
                self.apply_color(&icon, Some(icon_color))
                    .replace("\x1b[0m", "")
            } else {
                icon.clone()
            };

            let text_styled = self
                .apply_style(
                    &data.primary,
                    config.colors.text.as_ref(),
                    config.styles.text_bold,
                )
                .replace("\x1b[0m", "");

            let mut segment_content = format!(" {} {} ", icon_colored, text_styled);

            if !data.secondary.is_empty() {
                let secondary_styled = self
                    .apply_style(
                        &data.secondary,
                        config.colors.text.as_ref(),
                        config.styles.text_bold,
                    )
                    .replace("\x1b[0m", "");
                segment_content.push_str(&format!("{} ", secondary_styled));
            }

            // Apply background to the entire content and reset at the end
            format!("{}{}\x1b[49m", bg_code, segment_content)
        } else {
            // No background color, use original logic
            let icon_colored = self.apply_color(&icon, config.colors.icon.as_ref());
            let text_styled = self.apply_style(
                &data.primary,
                config.colors.text.as_ref(),
                config.styles.text_bold,
            );

            let mut segment = format!("{} {}", icon_colored, text_styled);

            if !data.secondary.is_empty() {
                segment.push_str(&format!(
                    " {}",
                    self.apply_style(
                        &data.secondary,
                        config.colors.text.as_ref(),
                        config.styles.text_bold
                    )
                ));
            }

            segment
        }
    }

    fn get_icon(&self, config: &SegmentConfig) -> String {
        match self.config.style.mode {
            StyleMode::Plain => config.icon.plain.clone(),
            StyleMode::NerdFont => config.icon.nerd_font.clone(),
            StyleMode::Powerline => config.icon.nerd_font.clone(), // Future: use Powerline icons
        }
    }

    fn apply_color(&self, text: &str, color: Option<&AnsiColor>) -> String {
        match color {
            Some(AnsiColor::Color16 { c16 }) => {
                let code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                format!("\x1b[{}m{}\x1b[0m", code, text)
            }
            Some(AnsiColor::Color256 { c256 }) => {
                format!("\x1b[38;5;{}m{}\x1b[0m", c256, text)
            }
            Some(AnsiColor::Rgb { r, g, b }) => {
                format!("\x1b[38;2;{};{};{}m{}\x1b[0m", r, g, b, text)
            }
            None => text.to_string(),
        }
    }

    fn apply_style(&self, text: &str, color: Option<&AnsiColor>, bold: bool) -> String {
        let mut codes = Vec::new();

        // Add style codes
        if bold {
            codes.push("1".to_string()); // Bold: \x1b[1m
        }

        // Add color codes
        match color {
            Some(AnsiColor::Color16 { c16 }) => {
                let color_code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                codes.push(color_code.to_string());
            }
            Some(AnsiColor::Color256 { c256 }) => {
                codes.push("38".to_string());
                codes.push("5".to_string());
                codes.push(c256.to_string());
            }
            Some(AnsiColor::Rgb { r, g, b }) => {
                codes.push("38".to_string());
                codes.push("2".to_string());
                codes.push(r.to_string());
                codes.push(g.to_string());
                codes.push(b.to_string());
            }
            None => {}
        }

        if codes.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{}m{}\x1b[0m", codes.join(";"), text)
        }
    }

    fn apply_background_color(&self, color: &AnsiColor) -> String {
        match color {
            AnsiColor::Color16 { c16 } => {
                let code = if *c16 < 8 { 40 + c16 } else { 100 + (c16 - 8) };
                format!("\x1b[{}m", code)
            }
            AnsiColor::Color256 { c256 } => {
                format!("\x1b[48;5;{}m", c256)
            }
            AnsiColor::Rgb { r, g, b } => {
                format!("\x1b[48;2;{};{};{}m", r, g, b)
            }
        }
    }

    /// Create a Powerline arrow with proper color transition
    fn create_powerline_arrow(
        &self,
        prev_bg: Option<&AnsiColor>,
        curr_bg: Option<&AnsiColor>,
    ) -> String {
        let arrow_char = "\u{e0b0}";

        match (prev_bg, curr_bg) {
            (Some(prev), Some(curr)) => {
                // Arrow foreground = previous segment's background
                // Arrow background = current segment's background
                let fg_code = self.color_to_foreground_code(prev);
                let bg_code = self.apply_background_color(curr);
                format!("{}{}{}\x1b[0m", bg_code, fg_code, arrow_char)
            }
            (Some(prev), None) => {
                // Previous segment has background, current doesn't
                let fg_code = self.color_to_foreground_code(prev);
                format!("{}{}\x1b[0m", fg_code, arrow_char)
            }
            (None, Some(curr)) => {
                // Current segment has background, previous doesn't
                let bg_code = self.apply_background_color(curr);
                format!("{}{}\x1b[0m", bg_code, arrow_char)
            }
            (None, None) => {
                // Neither segment has background color
                arrow_char.to_string()
            }
        }
    }

    /// Convert AnsiColor to foreground color code
    fn color_to_foreground_code(&self, color: &AnsiColor) -> String {
        match color {
            AnsiColor::Color16 { c16 } => {
                let code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                format!("\x1b[{}m", code)
            }
            AnsiColor::Color256 { c256 } => {
                format!("\x1b[38;5;{}m", c256)
            }
            AnsiColor::Rgb { r, g, b } => {
                format!("\x1b[38;2;{};{};{}m", r, g, b)
            }
        }
    }
}

pub fn collect_all_segments(
    config: &Config,
    input: &crate::config::InputData,
) -> Vec<(SegmentConfig, SegmentData)> {
    use crate::core::segments::*;

    let mut results = Vec::new();

    for segment_config in &config.segments {
        // Skip disabled segments to avoid unnecessary API requests
        if !segment_config.enabled {
            continue;
        }

        let segment_data = match segment_config.id {
            crate::config::SegmentId::Model => {
                let option = |key: &str| {
                    segment_config
                        .options
                        .get(key)
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true)
                };
                let segment = ModelSegment::new()
                    .with_effort(option("show_effort"))
                    .with_fast_mode(option("show_fast_mode"));
                segment.collect(input)
            }
            crate::config::SegmentId::Directory => {
                let segment = DirectorySegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Git => {
                let show_sha = segment_config
                    .options
                    .get("show_sha")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let segment = GitSegment::new().with_sha(show_sha);
                segment.collect(input)
            }
            crate::config::SegmentId::ContextWindow => {
                let segment = ContextWindowSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Usage => {
                let segment = UsageSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Cost => {
                let segment = CostSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Session => {
                let segment = SessionSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::OutputStyle => {
                let segment = OutputStyleSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Update => {
                let segment = UpdateSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::PromptCache => {
                let segment = PromptCacheSegment::new();
                segment.collect(input)
            }
        };

        if let Some(data) = segment_data {
            results.push((segment_config.clone(), data));
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes::ThemePresets;
    use std::collections::HashMap;

    #[test]
    fn width_skips_escape_sequences_and_counts_wide_characters() {
        assert_eq!(visible_width("\x1b[1;38;2;1;2;3mabc\x1b[0m"), 3);
        // OSC 8 hyperlinks, terminated by BEL or by ESC \
        assert_eq!(
            visible_width("\x1b]8;;https://github.com/a/b\x07link\x1b]8;;\x07"),
            4
        );
        assert_eq!(visible_width("\x1b]8;;https://x\x1b\\t\x1b]8;;\x1b\\"), 1);
        assert_eq!(visible_width("中文 💰"), 7);
    }

    #[test]
    fn width_left_by_claude_code() {
        assert_eq!(status_line_width("80", 0), Some(76));
        assert_eq!(status_line_width("80", 2), Some(72));
        assert_eq!(status_line_width("4", 0), None);
        assert_eq!(status_line_width("", 0), None);
    }

    #[test]
    fn wraps_between_segments_to_fit_the_width() {
        for theme in ["default", "powerline-dark"] {
            let config = ThemePresets::builtin_theme(theme);
            let segments: Vec<_> = config
                .segments
                .iter()
                .map(|segment| {
                    let mut segment = segment.clone();
                    segment.enabled = true;
                    let data = SegmentData {
                        primary: "x".repeat(10),
                        secondary: String::new(),
                        metadata: HashMap::new(),
                    };
                    (segment, data)
                })
                .collect();

            let single = StatusLineGenerator::new(config.clone()).generate(segments.clone());
            assert!(!single.contains('\n'), "{}", theme);
            assert!(visible_width(&single) > 40, "{}", theme);

            let wrapped = StatusLineGenerator::new(config)
                .with_max_width(Some(40))
                .generate(segments.clone());
            let lines: Vec<&str> = wrapped.lines().collect();
            assert!(lines.len() > 1, "{}", theme);
            for line in &lines {
                assert!(visible_width(line) <= 40, "{}: {:?}", theme, line);
            }
            // Every segment is still there
            assert_eq!(
                wrapped.matches(&"x".repeat(10)).count(),
                segments.len(),
                "{}",
                theme
            );
        }
    }
}
