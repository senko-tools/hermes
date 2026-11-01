//! A small multi-line text buffer with a cursor, for the message input and the `:` line.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    text: String,
    /// Byte offset, always on a char boundary.
    cursor: usize,
}

impl Input {
    pub fn with_text(text: &str) -> Self {
        Input {
            text: text.to_owned(),
            cursor: text.len(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
    }

    pub fn end(&mut self) {
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }

    /// Deletes the word before the cursor (Ctrl+W).
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// The word being typed: from the last whitespace before the cursor up to the cursor.
    pub fn current_token(&self) -> &str {
        let before = &self.text[..self.cursor];
        let start = before.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        &before[start..]
    }

    /// Replaces [`current_token`](Self::current_token) with `replacement`.
    pub fn replace_token(&mut self, replacement: &str) {
        let start = self.cursor - self.current_token().len();
        self.text.replace_range(start..self.cursor, replacement);
        self.cursor = start + replacement.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing() {
        let mut i = Input::default();
        for c in "héllo".chars() {
            i.insert(c);
        }
        i.left();
        i.left();
        i.backspace();
        assert_eq!(i.text(), "hélo");
        i.home();
        i.insert('>');
        i.end();
        i.insert_str(" wörld");
        assert_eq!(i.text(), ">hélo wörld");
        i.delete_word();
        assert_eq!(i.text(), ">hélo ");
        assert_eq!(i.take(), ">hélo ");
        assert!(i.is_empty());
    }

    #[test]
    fn tokens() {
        let mut i = Input::with_text("hi @al");
        assert_eq!(i.current_token(), "@al");
        i.replace_token("<@2> ");
        assert_eq!(i.text(), "hi <@2> ");
        let mut j = Input::with_text("line one\n:smi");
        assert_eq!(j.current_token(), ":smi");
        j.replace_token("😄 ");
        assert_eq!(j.text(), "line one\n😄 ");
    }
}
