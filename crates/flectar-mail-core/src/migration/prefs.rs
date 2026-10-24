//! Reader for Mozilla preference files (`prefs.js`, `user.js`).
//!
//! The files look like JavaScript but are a fixed statement grammar:
//! `user_pref("name", value);`. They are parsed as data and never evaluated.
//! Malformed statements are skipped the way Mozilla's own parser recovers,
//! so one damaged line does not hide every account.

use std::collections::HashMap;

const MAX_PREFS: usize = 200_000;
const MAX_ERRORS: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrefValue {
    String(String),
    Int(i64),
    Bool(bool),
}

#[derive(Debug, Default)]
pub(crate) struct Prefs {
    values: HashMap<String, PrefValue>,
}

impl Prefs {
    /// Parses one file. Returns `None` when nothing in it resembles a
    /// preference statement.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut prefs = Self::default();
        prefs.merge(text).then_some(prefs)
    }

    /// Applies the statements in `text` over the current values, as Mozilla
    /// applies `user.js` over `prefs.js`. Returns whether any were found.
    pub(crate) fn merge(&mut self, text: &str) -> bool {
        // Editors on Windows may prepend a byte order mark to `user.js`.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut parser = Parser {
            text: text.as_bytes(),
            source: text,
            at: 0,
            unterminated: usize::MAX,
        };
        let mut found = false;
        let mut errors = 0;
        while parser.skip_trivia() {
            let start = parser.at;
            match parser.statement() {
                Ok(Some((name, value))) => {
                    found = true;
                    if self.values.len() < MAX_PREFS || self.values.contains_key(&name) {
                        self.values.insert(name, value);
                    }
                }
                Ok(None) => found = true,
                Err(()) => {
                    errors += 1;
                    if errors > MAX_ERRORS {
                        break;
                    }
                    parser.at = start;
                    parser.recover();
                }
            }
        }
        found
    }

    pub(crate) fn string(&self, name: &str) -> Option<&str> {
        match self.values.get(name)? {
            PrefValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn int(&self, name: &str) -> Option<i64> {
        match self.values.get(name)? {
            PrefValue::Int(value) => Some(*value),
            // Some add-ons and older profiles store numbers as strings.
            PrefValue::String(value) => value.trim().parse().ok(),
            PrefValue::Bool(_) => None,
        }
    }

    pub(crate) fn bool(&self, name: &str) -> Option<bool> {
        match self.values.get(name)? {
            PrefValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Comma-separated key lists such as `mail.accountmanager.accounts`.
    pub(crate) fn list(&self, name: &str) -> Vec<String> {
        self.string(name)
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }
}

struct Parser<'a> {
    text: &'a [u8],
    source: &'a str,
    at: usize,
    /// Where a `/*` without an end was found. No comment after it ends, so
    /// recovering past it never searches the rest of the file again.
    unterminated: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    /// Skips whitespace and comments. Returns false at the end of input.
    fn skip_trivia(&mut self) -> bool {
        loop {
            match self.peek() {
                None => return false,
                Some(byte) if byte.is_ascii_whitespace() => self.at += 1,
                Some(b'#') => self.skip_line(),
                Some(b'/') if self.text.get(self.at + 1) == Some(&b'/') => self.skip_line(),
                Some(b'/') if self.text.get(self.at + 1) == Some(&b'*') => {
                    let end = (self.at < self.unterminated)
                        .then(|| self.source[self.at + 2..].find("*/"))
                        .flatten();
                    match end {
                        Some(end) => self.at += end + 4,
                        None => {
                            self.unterminated = self.unterminated.min(self.at);
                            self.at = self.text.len();
                        }
                    }
                }
                Some(_) => return true,
            }
        }
    }

    fn skip_line(&mut self) {
        while let Some(byte) = self.peek() {
            self.at += 1;
            if byte == b'\n' {
                break;
            }
        }
    }

    /// Moves past the next `;` (or line end) after a malformed statement.
    fn recover(&mut self) {
        self.at += 1;
        while let Some(byte) = self.peek() {
            self.at += 1;
            if byte == b';' || byte == b'\n' {
                break;
            }
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ()> {
        self.skip_trivia();
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(())
        }
    }

    fn identifier(&mut self) -> Result<&str, ()> {
        self.skip_trivia();
        let start = self.at;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            self.at += 1;
        }
        if self.at == start {
            Err(())
        } else {
            Ok(&self.source[start..self.at])
        }
    }

    /// One `function("name", value[, attribute...]);` statement. Statements
    /// other than preference definitions are accepted and ignored.
    fn statement(&mut self) -> Result<Option<(String, PrefValue)>, ()> {
        let function = self.identifier()?;
        let defines = matches!(function, "user_pref" | "pref" | "sticky_pref");
        self.expect(b'(')?;
        self.skip_trivia();
        let name = self.string()?;
        self.expect(b',')?;
        self.skip_trivia();
        let value = self.value()?;
        // Newer files may append attributes such as `locked` or `sticky`.
        loop {
            self.skip_trivia();
            if self.peek() != Some(b',') {
                break;
            }
            self.at += 1;
            self.identifier()?;
        }
        self.expect(b')')?;
        self.expect(b';')?;
        Ok(defines.then_some((name, value)))
    }

    fn value(&mut self) -> Result<PrefValue, ()> {
        match self.peek() {
            Some(b'"' | b'\'') => self.string().map(PrefValue::String),
            Some(b'-' | b'+' | b'0'..=b'9') => {
                let start = self.at;
                self.at += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    self.at += 1;
                }
                self.source[start..self.at]
                    .parse::<i64>()
                    .map(PrefValue::Int)
                    .map_err(|_| ())
            }
            _ => match self.identifier()? {
                "true" => Ok(PrefValue::Bool(true)),
                "false" => Ok(PrefValue::Bool(false)),
                _ => Err(()),
            },
        }
    }

    fn string(&mut self) -> Result<String, ()> {
        let quote = self
            .peek()
            .filter(|byte| matches!(byte, b'"' | b'\''))
            .ok_or(())?;
        self.at += 1;
        let mut value = String::new();
        loop {
            let rest = &self.source[self.at..];
            let end = rest.find([quote as char, '\\', '\n']).ok_or(())?;
            value.push_str(&rest[..end]);
            self.at += end;
            match self.peek().ok_or(())? {
                b'\n' => return Err(()),
                byte if byte == quote => {
                    self.at += 1;
                    return Ok(value);
                }
                _ => {
                    self.at += 1;
                    self.escape(&mut value)?;
                }
            }
        }
    }

    fn escape(&mut self, value: &mut String) -> Result<(), ()> {
        let byte = self.peek().ok_or(())?;
        self.at += 1;
        match byte {
            b'n' => value.push('\n'),
            b'r' => value.push('\r'),
            b't' => value.push('\t'),
            b'x' => {
                let code = self.hex(2)?;
                value.push(char::from_u32(code).ok_or(())?);
            }
            b'u' => {
                let high = self.hex(4)?;
                let code = if (0xD800..0xDC00).contains(&high)
                    && self.text.get(self.at..self.at + 2) == Some(b"\\u")
                {
                    self.at += 2;
                    let low = self.hex(4)?;
                    if !(0xDC00..0xE000).contains(&low) {
                        return Err(());
                    }
                    0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                } else {
                    high
                };
                value.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            // `\"`, `\'`, `\\` and any other escaped ASCII stand for themselves.
            byte if byte.is_ascii() => value.push(byte as char),
            _ => return Err(()),
        }
        Ok(())
    }

    fn hex(&mut self, digits: usize) -> Result<u32, ()> {
        let text = self.source.get(self.at..self.at + digits).ok_or(())?;
        let code = u32::from_str_radix(text, 16).map_err(|_| ())?;
        if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(());
        }
        self.at += digits;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unterminated_comments_do_not_hide_earlier_statements() {
        // Every line fails inside its statement, and recovery resumes after
        // it; the missing comment end is searched for once.
        let text = format!(
            "user_pref(\"kept\", 1);\n{}",
            "user_pref(\"x\", /*\n".repeat(5_000)
        );
        let prefs = Prefs::parse(&text).unwrap();
        assert_eq!(prefs.int("kept"), Some(1));
        assert_eq!(prefs.len(), 1);
    }

    #[test]
    fn parses_thunderbird_statements_and_escapes() {
        let prefs = Prefs::parse(
            r#"// Mozilla User Preferences
/* Do not edit this file while the application is running. */
user_pref("mail.server.server1.hostName", "imap.example.com");
user_pref("mail.server.server1.port", 993);
user_pref("mail.server.server1.offline_download", false);
user_pref("mail.identity.id1.htmlSigText", "Ana \"Q\" Ruiz\nC:\\mail é😀");
user_pref('single', 'it\'s');
pref("negative", -1, locked);
user_pref("ünïcode.raw", "Zürich");
"#,
        )
        .unwrap();
        assert_eq!(
            prefs.string("mail.server.server1.hostName"),
            Some("imap.example.com")
        );
        assert_eq!(prefs.int("mail.server.server1.port"), Some(993));
        assert_eq!(
            prefs.bool("mail.server.server1.offline_download"),
            Some(false)
        );
        assert_eq!(
            prefs.string("mail.identity.id1.htmlSigText"),
            Some("Ana \"Q\" Ruiz\nC:\\mail é😀")
        );
        assert_eq!(prefs.string("single"), Some("it's"));
        assert_eq!(prefs.int("negative"), Some(-1));
        assert_eq!(prefs.string("ünïcode.raw"), Some("Zürich"));
    }

    #[test]
    fn malformed_statements_do_not_hide_later_preferences() {
        let prefs = Prefs::parse(
            "user_pref(\"broken\", \"no end);\nuser_pref(\"a\" 1);\nnonsense\nuser_pref(\"kept\", 2);\n",
        )
        .unwrap();
        assert_eq!(prefs.int("kept"), Some(2));
        assert_eq!(prefs.len(), 1);
    }

    #[test]
    fn later_files_override_earlier_values_and_lists_split() {
        let mut prefs = Prefs::parse(
            "user_pref(\"mail.accountmanager.accounts\", \"account1, account2,,\");\nuser_pref(\"x\", 1);",
        )
        .unwrap();
        assert!(prefs.merge("user_pref(\"x\", 2);"));
        assert_eq!(prefs.int("x"), Some(2));
        assert_eq!(
            prefs.list("mail.accountmanager.accounts"),
            ["account1", "account2"]
        );
    }

    #[test]
    fn text_without_preferences_is_rejected() {
        assert!(Prefs::parse("{\"format\":\"something else\"}").is_none());
        assert!(Prefs::parse("").is_none());
        assert!(Prefs::parse("/* unterminated").is_none());
    }

    #[test]
    fn code_is_never_evaluated() {
        let prefs = Prefs::parse(
            "user_pref(\"a\", 1 + 2);\nuser_pref(\"b\", eval(\"x\"));\nuser_pref(\"c\", \"ok\");",
        )
        .unwrap();
        assert_eq!(prefs.int("a"), None);
        assert_eq!(prefs.string("b"), None);
        assert_eq!(prefs.string("c"), Some("ok"));
    }
}
