use super::utils::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

pub struct Composer {
    input: String,
    cursor: usize,
}

impl Composer {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
        }
    }

    pub fn height(&self) -> u16 {
        let lines = self.input.split('\n').count() as u16;
        (lines).max(1).min(10)
    }

    pub fn is_empty(&self) -> bool {
        self.input.trim().is_empty()
    }

    pub fn clear(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        // Pi style composer is completely borderless (App handles divider lines)
        let prefix = Span::styled(
            " › ",
            Style::default()
                .fg(if focused { PI_CYAN } else { PI_GRAY })
                .add_modifier(Modifier::BOLD),
        );

        let mut display_text = self.input.clone();
        if focused {
            if self.cursor >= display_text.len() {
                display_text.push('█'); // Solid block cursor like Pi
            } else {
                display_text.insert(self.cursor, '█');
            }
        }

        let p = Paragraph::new(Line::from(vec![prefix, Span::raw(display_text)]))
            .wrap(Wrap { trim: false });

        f.render_widget(p, area);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<String> {
        match key.code {
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                let msg = self.input.clone();
                if !msg.trim().is_empty() {
                    self.clear();
                    return Some(msg);
                }
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.input.insert(self.cursor, '\n');
                self.cursor += 1;
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.input.len();
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = previous_grapheme_boundary(&self.input, self.cursor);
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = next_grapheme_boundary(&self.input, self.cursor);
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.cursor = previous_word_boundary(&self.input, self.cursor);
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.cursor = next_word_boundary(&self.input, self.cursor);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.replace_range(..self.cursor, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.replace_range(self.cursor.., "");
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let new_cursor = previous_word_boundary(&self.input, self.cursor);
                self.input.replace_range(new_cursor..self.cursor, "");
                self.cursor = new_cursor;
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) => {
                let end = next_word_boundary(&self.input, self.cursor);
                self.input.replace_range(self.cursor..end, "");
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let new_cursor = previous_grapheme_boundary(&self.input, self.cursor);
                    self.input.replace_range(new_cursor..self.cursor, "");
                    self.cursor = new_cursor;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    let end = next_grapheme_boundary(&self.input, self.cursor);
                    self.input.replace_range(self.cursor..end, "");
                }
            }
            KeyCode::Left => {
                self.cursor = previous_grapheme_boundary(&self.input, self.cursor);
            }
            KeyCode::Right => {
                self.cursor = next_grapheme_boundary(&self.input, self.cursor);
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Composer;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn supports_multibyte_input_without_invalid_cursor_boundary() {
        let mut composer = Composer::new();

        assert!(composer.handle_key(key(KeyCode::Char('你'))).is_none());
        assert!(composer.handle_key(key(KeyCode::Char('好'))).is_none());

        let submitted = composer.handle_key(key(KeyCode::Enter));

        assert_eq!(submitted.as_deref(), Some("你好"));
    }
}

fn previous_grapheme_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut iter = text[cursor..].char_indices();
    let _ = iter.next();
    iter.next()
        .map(|(idx, _)| cursor + idx)
        .unwrap_or(text.len())
}

fn previous_word_boundary(text: &str, cursor: usize) -> usize {
    let mut seen_word = false;
    for (idx, ch) in text[..cursor].char_indices().rev() {
        if ch.is_whitespace() {
            if seen_word {
                return idx + ch.len_utf8();
            }
        } else {
            seen_word = true;
        }
    }
    0
}

fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let mut seen_word = false;
    for (offset, ch) in text[cursor..].char_indices() {
        let idx = cursor + offset;
        if ch.is_whitespace() {
            if seen_word {
                return idx;
            }
        } else {
            seen_word = true;
        }
    }
    text.len()
}
