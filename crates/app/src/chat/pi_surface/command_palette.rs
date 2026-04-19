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

pub struct CommandPalette {
    query: String,
    commands: Vec<(&'static str, &'static str, CommandAction)>,
    state: ListState,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            commands: vec![
                (
                    "/help",
                    "Show keyboard shortcuts and control-surface commands",
                    CommandAction::RunCommand("/help"),
                ),
                (
                    "/status",
                    "Inspect runtime posture and continuity settings",
                    CommandAction::RunCommand("/status"),
                ),
                (
                    "/history",
                    "Show the current transcript window",
                    CommandAction::RunCommand("/history"),
                ),
                (
                    "/compact",
                    "Create a manual continuity checkpoint",
                    CommandAction::RunCommand("/compact"),
                ),
                (
                    "/sessions",
                    "Inspect visible sessions rooted at the current scope",
                    CommandAction::RunCommand("/sessions"),
                ),
                (
                    "/workers",
                    "Inspect visible delegate worker sessions",
                    CommandAction::RunCommand("/workers"),
                ),
                (
                    "/review",
                    "Inspect the latest approval and review queue",
                    CommandAction::RunCommand("/review"),
                ),
                (
                    "/mission",
                    "Inspect mission-control lane counts and phase state",
                    CommandAction::RunCommand("/mission"),
                ),
                (
                    "/exit",
                    "Leave interactive chat",
                    CommandAction::RunCommand("/exit"),
                ),
            ],
            state: ListState::default(),
        }
    }

    pub fn show(&mut self, query: &str) {
        self.query = query.trim_start_matches('/').to_string();
        self.state.select(Some(0));
    }

    pub fn desired_height(&self) -> usize {
        let visible_items = self.filtered_commands().len().clamp(1, 5);
        visible_items + 2
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);

        let filtered = self.filtered_commands();
        let selected = self
            .state
            .selected()
            .unwrap_or(0)
            .min(filtered.len().saturating_sub(1));
        self.state.select(Some(selected));

        let label_width = filtered
            .iter()
            .map(|(cmd, _, _)| cmd.trim_start_matches('/').chars().count())
            .max()
            .unwrap_or(0)
            .clamp(8, 18);

        let result_items = filtered.iter().enumerate().map(|(index, (cmd, desc, _))| {
            let label = cmd.trim_start_matches('/');
            let is_selected = index == selected;
            let prefix = if is_selected { "→ " } else { "  " };
            let gap = " ".repeat(label_width.saturating_sub(label.chars().count()) + 2);
            let max_desc = area
                .width
                .saturating_sub((prefix.len() + label_width + 2) as u16)
                as usize;
            let desc = truncate(desc, max_desc);

            ListItem::new(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default().fg(if is_selected { PI_CYAN } else { PI_DIM_GRAY }),
                ),
                Span::styled(
                    label.to_owned(),
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
                Span::styled(desc, Style::default().fg(PI_GRAY)),
            ]))
        });

        let count_line = ListItem::new(Line::from(vec![Span::styled(
            format!("({}/{})", selected + 1, filtered.len().max(1)),
            Style::default().fg(PI_DIM_GRAY),
        )]));

        let items: Vec<ListItem> = result_items.chain(std::iter::once(count_line)).collect();

        let list = List::new(items).highlight_style(Style::default());
        f.render_stateful_widget(list, area, &mut self.state);
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
                let (_, _, action) = filtered.get(index)?;
                Some(*action)
            }
            KeyCode::Up => {
                let index = self.state.selected().unwrap_or(0).saturating_sub(1);
                self.state.select(Some(index));
                None
            }
            KeyCode::Down => {
                let max_index = self.filtered_commands().len().saturating_sub(1);
                let index = self
                    .state
                    .selected()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .min(max_index);
                self.state.select(Some(index));
                None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.state.select(Some(0));
                None
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.state.select(Some(0));
                None
            }
            _ => None,
        }
    }

    fn filtered_commands(&self) -> Vec<(&'static str, &'static str, CommandAction)> {
        self.commands
            .iter()
            .copied()
            .filter(|(cmd, desc, _)| {
                let query = self.query.as_str();
                query.is_empty() || cmd.contains(query) || desc.contains(query)
            })
            .collect()
    }
}

fn truncate(text: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max_len {
        return text.to_owned();
    }
    if max_len == 1 {
        return "…".to_owned();
    }
    let mut out = text.chars().take(max_len - 1).collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::{CommandAction, CommandPalette};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_uses_filtered_selection_instead_of_raw_index() {
        let mut palette = CommandPalette::new();
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
        let mut palette = CommandPalette::new();
        palette.show("/compactx");
        palette.handle_key(key(KeyCode::Backspace));

        let action = palette.handle_key(key(KeyCode::Enter));

        match action {
            Some(CommandAction::RunCommand("/compact")) => {}
            other => panic!("expected /compact action after backspace, got {other:?}"),
        }
    }
}
