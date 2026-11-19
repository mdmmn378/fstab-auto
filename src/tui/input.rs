//! A minimal single-line text field.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Default)]
pub struct TextInput {
    value: String,
    /// Cursor position in characters, 0..=len.
    cursor: usize,
}

impl TextInput {
    pub fn with_value(value: &str) -> Self {
        let mut input = Self::default();
        input.set(value);
        input
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn len(&self) -> usize {
        self.value.chars().count()
    }

    pub fn set(&mut self, value: &str) {
        self.value = value.to_string();
        self.cursor = self.len();
    }

    pub fn clear(&mut self) {
        self.set("");
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map_or(self.value.len(), |(i, _)| i)
    }

    pub fn insert(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        let at = self.byte_index(self.cursor);
        self.value.insert(at, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        text.chars().for_each(|c| self.insert(c));
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let at = self.byte_index(self.cursor);
            self.value.remove(at);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.len() {
            let at = self.byte_index(self.cursor);
            self.value.remove(at);
        }
    }

    /// Applies an editing key. Returns false when the key is not an editing
    /// key, so the caller can treat it as navigation.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => self.clear(),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len(),
            KeyCode::Char(_) if ctrl || key.modifiers.contains(KeyModifiers::ALT) => return false,
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            _ => return false,
        }
        true
    }

    /// The slice of the value that fits in `width` columns while keeping the
    /// cursor visible, plus the cursor's column inside that slice.
    pub fn visible(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }
        let start = (self.cursor + 1).saturating_sub(width);
        let text = self.value.chars().skip(start).take(width).collect();
        (text, self.cursor - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn editing_in_the_middle_handles_multibyte_text() {
        let mut input = TextInput::with_value("naïs");
        input.handle_key(key(KeyCode::Left));
        input.handle_key(key(KeyCode::Backspace));
        input.handle_key(key(KeyCode::Char('a')));
        assert_eq!(input.value(), "naas");
        input.handle_key(key(KeyCode::Home));
        input.handle_key(key(KeyCode::Delete));
        assert_eq!(input.value(), "aas");
        input.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(input.is_empty());
        assert!(!input.handle_key(key(KeyCode::Enter)));
    }

    #[test]
    fn visible_window_follows_the_cursor() {
        let input = TextInput::with_value("abcdefghij");
        assert_eq!(input.visible(20), ("abcdefghij".to_string(), 10));
        assert_eq!(input.visible(5), ("ghij".to_string(), 4));
    }
}
