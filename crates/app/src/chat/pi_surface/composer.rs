use super::utils::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;

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
        let prefix = Span::styled(
            " › ",
            Style::default()
                .fg(if focused { PI_CYAN } else { PI_GRAY })
                .add_modifier(Modifier::BOLD),
        );

        let p = Paragraph::new(Line::from(vec![prefix, Span::raw(self.input.clone())]))
            .wrap(Wrap { trim: false });

        f.render_widget(p, area);
    }

    pub fn cursor_position(&self, area: Rect) -> (u16, u16) {
        let prefix_width = 3usize;
        let available_width = area.width.saturating_sub(prefix_width as u16).max(1) as usize;
        let mut row = 0usize;
        let mut line_col = 0usize;

        for grapheme in self.input[..self.cursor].graphemes(true) {
            if grapheme == "\n" {
                row += 1;
                line_col = 0;
                continue;
            }

            let ch_width = display_width(ch);
            if line_col + ch_width > available_width {
                row += 1;
                line_col = 0;
            }
            line_col += ch_width;
        }

        let col = if row == 0 {
            prefix_width + line_col
        } else {
            line_col
        };

        (
            area.x + col.min(area.width.saturating_sub(1) as usize) as u16,
            area.y + row.min(area.height.saturating_sub(1) as usize) as u16,
        )
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
            KeyCode::Home => {
                self.cursor = line_start_boundary(&self.input, self.cursor);
            }
            KeyCode::End => {
                self.cursor = line_end_boundary(&self.input, self.cursor);
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
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn supports_multibyte_input_without_invalid_cursor_boundary() {
        let mut composer = Composer::new();

        assert!(composer.handle_key(key(KeyCode::Char('你'))).is_none());
        assert!(composer.handle_key(key(KeyCode::Char('好'))).is_none());

        let submitted = composer.handle_key(key(KeyCode::Enter));

        assert_eq!(submitted.as_deref(), Some("你好"));
    }

    #[test]
    fn cursor_position_respects_prefix_before_wrapping() {
        let mut composer = Composer::new();
        assert!(composer.handle_key(key(KeyCode::Char('a'))).is_none());
        assert!(composer.handle_key(key(KeyCode::Char('b'))).is_none());

        assert_eq!(composer.cursor_position(Rect::new(0, 0, 6, 3)), (5, 0));

        assert!(composer.handle_key(key(KeyCode::Char('c'))).is_none());
        assert_eq!(composer.cursor_position(Rect::new(0, 0, 6, 3)), (5, 0));

        assert!(composer.handle_key(key(KeyCode::Char('d'))).is_none());

        assert_eq!(composer.cursor_position(Rect::new(0, 0, 6, 3)), (1, 1));
    }

    #[test]
    fn word_motion_and_delete_shortcuts_keep_cursor_on_valid_boundaries() {
        let mut composer = Composer::new();
        for ch in "foo 你好 bar".chars() {
            assert!(composer.handle_key(key(KeyCode::Char(ch))).is_none());
        }

        let submitted = composer.handle_key(key(KeyCode::Enter));
        assert_eq!(submitted.as_deref(), Some("alpha\nXbetaY"));
    }
}

fn previous_grapheme_boundary(text: &str, cursor: usize) -> usize {
    UnicodeSegmentation::grapheme_indices(text, true)
        .take_while(|(idx, _)| *idx < cursor)
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }

    UnicodeSegmentation::grapheme_indices(text, true)
        .find_map(|(idx, _grapheme)| {
            if idx <= cursor {
                return None;
            }
            Some(idx)
        })
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

fn line_start_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0)
}

fn line_end_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}

fn display_width(grapheme: &str) -> usize {
    if grapheme.is_ascii() {
        grapheme.chars().count().max(1)
    } else {
        grapheme
            .chars()
            .map(|ch| if ch.is_ascii() { 1 } else { 2 })
            .sum::<usize>()
            .max(2)
    }
}
