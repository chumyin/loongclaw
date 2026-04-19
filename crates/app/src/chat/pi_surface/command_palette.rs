use crate::chat::pi_surface::i18n::{I18nService, Language, PiCopy};
use crate::chat::pi_surface::utils::*;
use crate::chat::pi_surface::i18n::SURFACE_COMMANDS;
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
    i18n: I18nService,
}

impl CommandPalette {
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
                    command: "/fast_lane_summary",
                    label: PiCopy::CommandDeckLabelFastLane,
                    description: PiCopy::CommandDeckDescFastLane,
                    action: CommandAction::RunCommand("/fast_lane_summary"),
                },
                CommandEntry {
                    command: "/safe_lane_summary",
                    label: PiCopy::CommandDeckLabelSafeLane,
                    description: PiCopy::CommandDeckDescSafeLane,
                    action: CommandAction::RunCommand("/safe_lane_summary"),
                },
                CommandEntry {
                    command: "/turn_checkpoint_summary",
                    label: PiCopy::CommandDeckLabelCheckpoint,
                    description: PiCopy::CommandDeckDescCheckpoint,
                    action: CommandAction::RunCommand("/turn_checkpoint_summary"),
                },
                CommandEntry {
                    command: "/turn_checkpoint_repair",
                    label: PiCopy::CommandDeckLabelRepair,
                    description: PiCopy::CommandDeckDescRepair,
                    action: CommandAction::RunCommand("/turn_checkpoint_repair"),
                },
                CommandEntry {
                    command: "/exit",
                    label: PiCopy::CommandDeckLabelExit,
                    description: PiCopy::CommandDeckDescExit,
                    action: CommandAction::RunCommand("/exit"),
                },
            ],
            state: ListState::default(),
            i18n: I18nService::new(lang),
        }
    }

    pub fn show(&mut self, query: &str) {
        self.query = query
            .trim()
            .trim_start_matches(['/', ':'])
            .to_string();
        self.state.select(Some(0));
    }

    pub fn desired_height(&self) -> usize {
        let visible_items = self.filtered_commands().len().clamp(1, 6);
        visible_items + 2
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);

        let filtered = self.filtered_commands();
        if filtered.is_empty() {
            let items = vec![
                ListItem::new(Line::from(vec![Span::styled(
                    format!("  {}", self.i18n.text(PiCopy::CommandDeckEmpty)),
                    Style::default().fg(PI_DIM_GRAY),
                )])),
                ListItem::new(Line::from(vec![Span::styled(
                    "(0/0)",
                    Style::default().fg(PI_DIM_GRAY),
                )])),
            ];
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
        let max_visible = area.height.saturating_sub(1) as usize;
        let start = selected.saturating_sub(max_visible.saturating_sub(1));
        let end = (start + max_visible).min(filtered.len());
        let visible = &filtered[start..end];

        let label_width = filtered
            .iter()
            .map(|entry| self.display_label(entry).chars().count())
            .max()
            .unwrap_or(0)
            .clamp(8, 18);

        let result_items = visible.iter().enumerate().map(|(visible_index, entry)| {
            let index = start + visible_index;
            let label = self.display_label(entry);
            let is_selected = index == selected;
            let prefix = if is_selected { "→ " } else { "  " };
            let gap = " ".repeat(label_width.saturating_sub(label.chars().count()) + 2);
            let max_desc = area
                .width
                .saturating_sub((prefix.len() + label_width + 2) as u16)
                as usize;
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
                filtered.get(index).map(|entry| entry.action)
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
            KeyCode::Char(':') if self.query.is_empty() => None,
            KeyCode::Char(c) => {
                self.query.push(c);
                self.state.select(Some(0));
                None
            }
            _ => None,
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
}
