use crate::config::{Config, SegmentId};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Panel {
    SegmentList,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldSelection {
    Enabled,
    Icon,
    IconColor,
    TextColor,
    BackgroundColor,
    TextStyle,
    /// The segment's option at this position in `SegmentId::options`
    Option(usize),
}

impl FieldSelection {
    /// The settings panel's rows for a segment, top to bottom
    pub fn all(id: SegmentId) -> Vec<FieldSelection> {
        let mut fields = vec![
            FieldSelection::Enabled,
            FieldSelection::Icon,
            FieldSelection::IconColor,
            FieldSelection::TextColor,
            FieldSelection::BackgroundColor,
            FieldSelection::TextStyle,
        ];
        fields.extend((0..id.options().len()).map(FieldSelection::Option));
        fields
    }
}

#[derive(Default)]
pub struct SegmentListComponent;

impl SegmentListComponent {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        config: &Config,
        selected_segment: usize,
        selected_panel: &Panel,
    ) {
        let items: Vec<ListItem> = config
            .segments
            .iter()
            .enumerate()
            .map(|(i, segment)| {
                let is_selected = i == selected_segment && *selected_panel == Panel::SegmentList;
                let enabled_marker = if segment.enabled { "●" } else { "○" };
                let segment_name = segment.id.name();

                if is_selected {
                    // Selected item with colored cursor
                    ListItem::new(Line::from(vec![
                        Span::styled("▶ ", Style::default().fg(Color::Cyan)),
                        Span::raw(format!("{} {}", enabled_marker, segment_name)),
                    ]))
                } else {
                    // Non-selected item
                    ListItem::new(format!("  {} {}", enabled_marker, segment_name))
                }
            })
            .collect();
        let segments_block = Block::default()
            .borders(Borders::ALL)
            .title("Segments")
            .border_style(if *selected_panel == Panel::SegmentList {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            });
        let segments_list = List::new(items).block(segments_block);
        // The state scrolls the list to keep the selected segment visible
        let mut state = ListState::default().with_selected(Some(selected_segment));
        f.render_stateful_widget(segments_list, area, &mut state);
    }
}
