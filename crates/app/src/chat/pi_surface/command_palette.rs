use crate::chat::pi_surface::i18n::{I18nService, Language, PiCopy};
use crate::chat::pi_surface::utils::*;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    RunCommand(&'static str),
    Close,
}

#[derive(Debug, Clone, Copy)]
struct CommandEntry {
    command: &'static str,
    label: PiCopy,
    description: PiCopy,
    action: CommandAction,
}

pub struct CommandPalette {
    query: String,
    commands: Vec<CommandEntry>,
    state: ListState,
    scroll_offset: usize,
    i18n: I18nService,
}

impl CommandPalette {
    const VISIBLE_ROWS: usize = 7;
    const FOOTER_ROWS: usize = 1;

    pub fn new(lang: Language) -> Self {
        Self {
            query: String::new(),
            commands: vec![
                CommandEntry {
                    command: "/help",
                    label: PiCopy::CommandDeckLabelHelp,
                    description: PiCopy::CommandDeckDescHelp,
                    action: CommandAction::RunCommand("/help"),
                },
                CommandEntry {
                    command: "/status",
                    label: PiCopy::CommandDeckLabelStatus,
                    description: PiCopy::CommandDeckDescStatus,
                    action: CommandAction::RunCommand("/status"),
                },
                CommandEntry {
                    command: "/history",
                    label: PiCopy::CommandDeckLabelHistory,
                    description: PiCopy::CommandDeckDescHistory,
                    action: CommandAction::RunCommand("/history"),
                },
                CommandEntry {
                    command: "/compact",
                    label: PiCopy::CommandDeckLabelCompact,
                    description: PiCopy::CommandDeckDescCompact,
                    action: CommandAction::RunCommand("/compact"),
                },
                CommandEntry {
                    command: "/sessions",
                    label: PiCopy::CommandDeckLabelSessions,
                    description: PiCopy::CommandDeckDescSessions,
                    action: CommandAction::RunCommand("/sessions"),
                },
                CommandEntry {
                    command: "/workers",
                    label: PiCopy::CommandDeckLabelWorkers,
                    description: PiCopy::CommandDeckDescWorkers,
                    action: CommandAction::RunCommand("/workers"),
                },
                CommandEntry {
                    command: "/review",
                    label: PiCopy::CommandDeckLabelReview,
                    description: PiCopy::CommandDeckDescReview,
                    action: CommandAction::RunCommand("/review"),
                },
                CommandEntry {
                    command: "/mission",
                    label: PiCopy::CommandDeckLabelMission,
                    description: PiCopy::CommandDeckDescMission,
                    action: CommandAction::RunCommand("/mission"),
                },
                CommandEntry {
                    command: "/exit",
                    label: PiCopy::CommandDeckLabelExit,
                    description: PiCopy::CommandDeckDescExit,
                    action: CommandAction::RunCommand("/exit"),
                },
            ],
            state: ListState::default(),
            scroll_offset: 0,
            i18n: I18nService::new(lang),
        }
    }

    pub fn show(&mut self, query: &str) {
        self.query = query.trim().trim_start_matches(['/', ':']).to_string();
        self.state.select(Some(0));
        self.scroll_offset = 0;
    }

    pub fn desired_height(&self) -> usize {
        Self::visible_rows_for_total(self.filtered_commands().len()) + Self::FOOTER_ROWS
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);

        let filtered = self.filtered_commands();
        let visible_rows = Self::visible_rows_for_total(filtered.len());
        if filtered.is_empty() {
            let mut items = vec![ListItem::new(Line::from(vec![Span::styled(
                format!("  {}", self.i18n.text(PiCopy::CommandDeckEmpty)),
                Style::default().fg(PI_DIM_GRAY),
            )]))];
            while items.len() < visible_rows {
                items.push(ListItem::new(Line::from("")));
            }
            items.push(ListItem::new(Line::from(vec![Span::styled(
                "(0/0)",
                Style::default().fg(PI_DIM_GRAY),
            )])));
            let list = List::new(items).highlight_style(Style::default());
            f.render_stateful_widget(list, area, &mut self.state);
            return;
        }

        let selected = self
            .state
            .selected()
            .unwrap_or(0)
            .min(filtered.len().saturating_sub(1));
        self.state.select(Some(selected));
        self.sync_scroll(selected, filtered.len());
        let start = self.scroll_offset.min(filtered.len().saturating_sub(1));
        let end = (start + visible_rows).min(filtered.len());
        let visible = filtered.get(start..end).unwrap_or(&[]);

        let label_width = filtered
            .iter()
            .map(|entry| crate::presentation::display_width(self.display_label(entry).as_str()))
            .max()
            .unwrap_or(0)
            .clamp(8, 18);

        let mut items: Vec<ListItem> = visible
            .iter()
            .enumerate()
            .map(|(visible_index, entry)| {
                let index = start + visible_index;
                let label = self.display_label(entry);
                let is_selected = index == selected;
                let prefix = if is_selected { "→ " } else { "  " };
                let gap = " ".repeat(
                    label_width.saturating_sub(crate::presentation::display_width(&label)) + 2,
                );
                let max_desc = area.width.saturating_sub(
                    (crate::presentation::display_width(prefix) + label_width + 2) as u16,
                ) as usize;
                let desc = truncate(self.i18n.text(entry.description), max_desc);

                ListItem::new(Line::from(vec![
                    Span::styled(
                        prefix,
                        Style::default().fg(if is_selected { PI_CYAN } else { PI_DIM_GRAY }),
                    ),
                    Span::styled(
                        label,
                        Style::default()
                            .fg(if is_selected {
                                PI_CYAN
                            } else {
                                ratatui::style::Color::White
                            })
                            .add_modifier(if is_selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                    Span::raw(gap),
                    Span::styled(
                        desc,
                        Style::default().fg(if is_selected { PI_ACCENT } else { PI_GRAY }),
                    ),
                ]))
            })
            .collect();

        while items.len() < visible_rows {
            items.push(ListItem::new(Line::from("")));
        }

        let count_line = ListItem::new(Line::from(vec![Span::styled(
            format!("({}/{})", selected + 1, filtered.len().max(1)),
            Style::default().fg(PI_DIM_GRAY),
        )]));

        items.push(count_line);
        let list = List::new(items).highlight_style(Style::default());
        let mut visible_state = ListState::default();
        visible_state.select(Some(selected.saturating_sub(start)));
        f.render_stateful_widget(list, area, &mut visible_state);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<CommandAction> {
        match key.code {
            KeyCode::Esc => Some(CommandAction::Close),
            KeyCode::Enter => {
                let filtered = self.filtered_commands();
                let index = self
                    .state
                    .selected()
                    .unwrap_or(0)
                    .min(filtered.len().saturating_sub(1));
                filtered.get(index).map(|entry| entry.action)
            }
            KeyCode::Up => {
                let total = self.filtered_commands().len();
                if total == 0 {
                    return None;
                }
                let current = self.state.selected().unwrap_or(0).min(total - 1);
                let index = if current == 0 { total - 1 } else { current - 1 };
                self.state.select(Some(index));
                self.sync_scroll(index, total);
                None
            }
            KeyCode::Down => {
                let total = self.filtered_commands().len();
                if total == 0 {
                    return None;
                }
                let current = self.state.selected().unwrap_or(0).min(total - 1);
                let index = if current + 1 >= total { 0 } else { current + 1 };
                self.state.select(Some(index));
                self.sync_scroll(index, total);
                None
            }
            KeyCode::PageUp => {
                let total = self.filtered_commands().len();
                if total == 0 {
                    return None;
                }
                let page = Self::visible_rows_for_total(total).max(1);
                let current = self.state.selected().unwrap_or(0).min(total - 1);
                let index = current.saturating_sub(page);
                self.state.select(Some(index));
                self.sync_scroll(index, total);
                None
            }
            KeyCode::PageDown => {
                let total = self.filtered_commands().len();
                if total == 0 {
                    return None;
                }
                let page = Self::visible_rows_for_total(total).max(1);
                let current = self.state.selected().unwrap_or(0).min(total - 1);
                let index = (current + page).min(total - 1);
                self.state.select(Some(index));
                self.sync_scroll(index, total);
                None
            }
            KeyCode::Home => {
                let total = self.filtered_commands().len();
                if total == 0 {
                    return None;
                }
                self.state.select(Some(0));
                self.sync_scroll(0, total);
                None
            }
            KeyCode::End => {
                let total = self.filtered_commands().len();
                if total == 0 {
                    return None;
                }
                let index = total - 1;
                self.state.select(Some(index));
                self.sync_scroll(index, total);
                None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.state.select(Some(0));
                self.scroll_offset = 0;
                None
            }
            KeyCode::Char(':') if self.query.is_empty() => None,
            KeyCode::Char(c) => {
                self.query.push(c);
                self.state.select(Some(0));
                self.scroll_offset = 0;
                None
            }
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Delete
            | KeyCode::Insert
            | KeyCode::F(_)
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => None,
        }
    }

    fn filtered_commands(&self) -> Vec<CommandEntry> {
        let query = self.query.trim().to_ascii_lowercase();
        self.commands
            .iter()
            .copied()
            .filter(|entry| {
                if query.is_empty() {
                    return true;
                }
                let command = entry.command.to_ascii_lowercase();
                let label = self.i18n.text(entry.label).to_ascii_lowercase();
                let desc = self.i18n.text(entry.description).to_ascii_lowercase();
                command.contains(query.as_str())
                    || label.contains(query.as_str())
                    || desc.contains(query.as_str())
            })
            .collect()
    }

    fn display_label(&self, entry: &CommandEntry) -> String {
        self.i18n.text(entry.label).to_owned()
    }

    fn sync_scroll(&mut self, selected: usize, total: usize) {
        let visible_rows = Self::visible_rows_for_total(total);
        if total <= visible_rows {
            self.scroll_offset = 0;
            return;
        }

        if selected < self.scroll_offset {
            self.scroll_offset = selected;
        } else if selected >= self.scroll_offset + visible_rows {
            self.scroll_offset = selected + 1 - visible_rows;
        }
    }

    fn visible_rows_for_total(total: usize) -> usize {
        total.clamp(1, Self::VISIBLE_ROWS)
    }
}

fn truncate(text: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    let count = crate::presentation::display_width(text);
    if count <= max_len {
        return text.to_owned();
    }
    if max_len == 1 {
        return "…".to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = crate::presentation::char_display_width(ch);
        if used + width > max_len.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::{CommandAction, CommandPalette};
    use crate::chat::pi_surface::i18n::Language;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_uses_filtered_selection_instead_of_raw_index() {
        let mut palette = CommandPalette::new(Language::En);
        palette.show("/h");
        palette.handle_key(key(KeyCode::Down));

        let action = palette.handle_key(key(KeyCode::Enter));

        match action {
            Some(CommandAction::RunCommand("/history")) => {}
            other => panic!("expected /history action, got {other:?}"),
        }
    }

    #[test]
    fn backspace_updates_query_without_panic() {
        let mut palette = CommandPalette::new(Language::En);
        palette.show("/compactx");
        palette.handle_key(key(KeyCode::Backspace));

        let action = palette.handle_key(key(KeyCode::Enter));

        match action {
            Some(CommandAction::RunCommand("/compact")) => {}
            other => panic!("expected /compact action after backspace, got {other:?}"),
        }
    }

    #[test]
    fn localized_query_matches_translated_labels() {
        let mut palette = CommandPalette::new(Language::ZhCn);
        palette.show("帮助");

        let action = palette.handle_key(key(KeyCode::Enter));

        match action {
            Some(CommandAction::RunCommand("/help")) => {}
            other => panic!("expected localized /help action, got {other:?}"),
        }
    }

    #[test]
    fn desired_height_shrinks_with_filtered_results() {
        let mut palette = CommandPalette::new(Language::En);
        palette.show("/mis");
        assert_eq!(palette.desired_height(), 2);

        palette.show("");
        assert_eq!(palette.desired_height(), 8);
    }

    #[test]
    fn desired_height_for_no_matches_keeps_only_result_and_footer() {
        let mut palette = CommandPalette::new(Language::En);
        palette.show("zzz-no-match");
        assert_eq!(palette.desired_height(), 2);
    }

    #[test]
    fn truncate_respects_display_cell_width_for_cjk() {
        let truncated = super::truncate("帮助命令说明", 5);

        assert!(crate::presentation::display_width(&truncated) <= 5);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn page_navigation_moves_by_visible_rows() {
        let mut palette = CommandPalette::new(Language::En);
        palette.show("");

        let _ = palette.handle_key(key(KeyCode::PageDown));
        match palette.handle_key(key(KeyCode::Enter)) {
            Some(CommandAction::RunCommand("/mission")) => {}
            other => panic!("expected page-down to land on /mission, got {other:?}"),
        }

        let _ = palette.handle_key(key(KeyCode::PageUp));
        match palette.handle_key(key(KeyCode::Enter)) {
            Some(CommandAction::RunCommand("/help")) => {}
            other => panic!("expected page-up to return to /help, got {other:?}"),
        }
    }

    #[test]
    fn home_and_end_jump_to_extremes() {
        let mut palette = CommandPalette::new(Language::En);
        palette.show("");

        let _ = palette.handle_key(key(KeyCode::End));
        match palette.handle_key(key(KeyCode::Enter)) {
            Some(CommandAction::RunCommand("/exit")) => {}
            other => panic!("expected end to land on /exit, got {other:?}"),
        }

        let _ = palette.handle_key(key(KeyCode::Home));
        match palette.handle_key(key(KeyCode::Enter)) {
            Some(CommandAction::RunCommand("/help")) => {}
            other => panic!("expected home to land on /help, got {other:?}"),
        }
    }

    #[test]
    fn arrow_navigation_wraps_at_edges() {
        let mut palette = CommandPalette::new(Language::En);
        palette.show("");

        let action = palette.handle_key(key(KeyCode::Up));
        match action {
            None => {}
            other => panic!("unexpected action while wrapping up: {other:?}"),
        }
        match palette.handle_key(key(KeyCode::Enter)) {
            Some(CommandAction::RunCommand("/exit")) => {}
            other => panic!("expected wrap-up to land on /exit, got {other:?}"),
        }

        let mut palette = CommandPalette::new(Language::En);
        palette.show("");
        for _ in 0..8 {
            let _ = palette.handle_key(key(KeyCode::Down));
        }
        match palette.handle_key(key(KeyCode::Enter)) {
            Some(CommandAction::RunCommand("/exit")) => {}
            other => panic!("expected repeated down to reach /exit, got {other:?}"),
        }
        let _ = palette.handle_key(key(KeyCode::Down));
        match palette.handle_key(key(KeyCode::Enter)) {
            Some(CommandAction::RunCommand("/help")) => {}
            other => panic!("expected wrap-down to return to /help, got {other:?}"),
        }
    }
}
