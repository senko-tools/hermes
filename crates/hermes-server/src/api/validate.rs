//! Input validation (PLAN.md section 13). Problems are collected per field and returned together
//! as one `VALIDATION_FAILED` error.

use std::collections::BTreeMap;

use hermes_proto::ApiError;

use super::Error;

/// Longest accepted password, so hashing cannot be used to burn CPU.
pub const MAX_PASSWORD_CHARS: usize = 256;
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

#[derive(Debug, Default)]
pub struct Fields(BTreeMap<String, String>);

impl Fields {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the problem if `result` is an error; returns the value otherwise.
    pub fn check<T>(&mut self, field: &str, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(problem) => {
                self.0.entry(field.to_owned()).or_insert(problem);
                None
            }
        }
    }

    pub fn add(&mut self, field: &str, problem: impl Into<String>) {
        self.0
            .entry(field.to_owned())
            .or_insert_with(|| problem.into());
    }

    /// `Ok` if no problem was recorded.
    pub fn finish(self) -> Result<(), Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error(ApiError::validation(self.0)))
        }
    }
}

/// A `VALIDATION_FAILED` error for a single field.
pub fn field_error(field: &str, problem: impl Into<String>) -> Error {
    let mut fields = Fields::new();
    fields.add(field, problem);
    Error(ApiError::validation(fields.0))
}

/// Trims and lowercases, then requires `^[a-z0-9_.]{2,32}$`.
pub fn username(raw: &str) -> Result<String, String> {
    let name = raw.trim().to_lowercase();
    if !(2..=32).contains(&name.chars().count()) {
        return Err("must be 2 to 32 characters".into());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
    {
        return Err("may only contain letters a-z, digits, '_' and '.'".into());
    }
    Ok(name)
}

pub fn password(raw: &str, min_chars: u32) -> Result<(), String> {
    let len = raw.chars().count();
    let min = min_chars as usize;
    if len < min {
        return Err(format!("must be at least {min} characters"));
    }
    if len > MAX_PASSWORD_CHARS {
        return Err(format!("must be at most {MAX_PASSWORD_CHARS} characters"));
    }
    Ok(())
}

/// Bidirectional overrides, embeddings and isolates: in a name or filename they reorder what
/// follows, so `invoice<U+202E>fdp.exe` reads as `invoiceexe.pdf` and a display name can look
/// like someone else's. (Zero-width joiners stay allowed: emoji need them.)
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Why `name` is not acceptable as a single-line name or filename, if it is not.
pub fn name_characters(name: &str) -> Result<(), String> {
    if name.chars().any(char::is_control) {
        return Err("must not contain control characters".into());
    }
    if name.chars().any(is_bidi_control) {
        return Err("must not contain text direction overrides".into());
    }
    Ok(())
}

/// Trims, then requires 1 to 64 characters without control characters.
pub fn device_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if !(1..=MAX_DEVICE_NAME_CHARS).contains(&name.chars().count()) {
        return Err(format!("must be 1 to {MAX_DEVICE_NAME_CHARS} characters"));
    }
    name_characters(name)?;
    Ok(name.to_owned())
}

pub const SPACE_NAME_CHARS: (usize, usize) = (2, 64);
pub const SPACE_DESCRIPTION_CHARS: usize = 512;
pub const ROLE_NAME_CHARS: (usize, usize) = (1, 32);
pub const CHANNEL_NAME_CHARS: (usize, usize) = (1, 100);
pub const CHANNEL_TOPIC_CHARS: usize = 1024;
pub const NICKNAME_CHARS: (usize, usize) = (1, 32);
pub const BAN_REASON_CHARS: usize = 512;

/// A single-line name: trims, then requires `min..=max` characters without control characters
/// or text direction overrides.
pub fn name(raw: &str, (min, max): (usize, usize)) -> Result<String, String> {
    let name = raw.trim();
    if !(min..=max).contains(&name.chars().count()) {
        return Err(format!("must be {min} to {max} characters"));
    }
    name_characters(name)?;
    Ok(name.to_owned())
}

/// Optional free text (descriptions, topics, reasons): trims; empty means `None`; at most `max`
/// characters; no control characters except `\n` and `\t`.
pub fn optional_text(raw: &str, max: usize) -> Result<Option<String>, String> {
    let text = raw.trim();
    if text.chars().count() > max {
        return Err(format!("must be at most {max} characters"));
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err("must not contain control characters".into());
    }
    Ok((!text.is_empty()).then(|| text.to_owned()))
}

/// Message content (raw markup, not trimmed): at most `max` characters; no control characters
/// except `\n` and `\t` (PLAN.md section 13).
pub fn message_content(raw: &str, max: usize) -> Result<(), String> {
    if raw.chars().count() > max {
        return Err(format!("must be at most {max} characters"));
    }
    if raw
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err("must not contain control characters other than newlines and tabs".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_contents() {
        assert!(message_content("  hi\n\tthere ", 20).is_ok());
        assert!(message_content("", 20).is_ok());
        assert!(message_content(&"é".repeat(20), 20).is_ok());
        assert!(message_content(&"é".repeat(21), 20).is_err());
        assert!(message_content("a\r\nb", 20).is_err());
        assert!(message_content("a\u{1b}[31m", 20).is_err());
    }

    #[test]
    fn names() {
        assert_eq!(name("  general ", (1, 100)).unwrap(), "general");
        assert!(name(" ", (1, 100)).is_err());
        assert!(name("a", SPACE_NAME_CHARS).is_err());
        assert!(name("a\tb", (1, 100)).is_err());
        assert!(name(&"é".repeat(32), ROLE_NAME_CHARS).is_ok());
        assert!(name(&"é".repeat(33), ROLE_NAME_CHARS).is_err());
    }

    #[test]
    fn optional_texts() {
        assert_eq!(optional_text("  ", 10).unwrap(), None);
        assert_eq!(
            optional_text(" a\nb\t", 10).unwrap().as_deref(),
            Some("a\nb")
        );
        assert!(optional_text("a\u{7}", 10).is_err());
        assert!(optional_text(&"x".repeat(11), 10).is_err());
    }

    #[test]
    fn usernames() {
        assert_eq!(username("  Senko.San_1 ").unwrap(), "senko.san_1");
        assert!(username("a").is_err());
        assert!(username(&"a".repeat(33)).is_err());
        assert!(username(&"a".repeat(32)).is_ok());
        assert!(username("sen ko").is_err());
        assert!(username("senkö").is_err());
        assert!(username("sen-ko").is_err());
    }

    #[test]
    fn passwords() {
        assert!(password("1234567", 8).is_err());
        assert!(password("12345678", 8).is_ok());
        assert!(
            password("ääääääää", 8).is_ok(),
            "counts characters, not bytes"
        );
        assert!(password(&"x".repeat(257), 8).is_err());
    }

    #[test]
    fn device_names() {
        assert_eq!(device_name("  Desktop PC ").unwrap(), "Desktop PC");
        assert!(device_name("   ").is_err());
        assert!(device_name("a\nb").is_err());
        assert!(device_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn fields_keep_the_first_problem() {
        let mut fields = Fields::new();
        assert_eq!(fields.check("a", Ok::<_, String>(1)), Some(1));
        assert!(Fields::new().finish().is_ok());
        fields.add("b", "first");
        fields.add("b", "second");
        let err = fields.finish().unwrap_err().0;
        assert_eq!(err.fields.unwrap()["b"], "first");
    }
}
