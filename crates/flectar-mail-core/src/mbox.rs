//! Mail as mbox (RFC 4155): each message follows a `From ` line.
//!
//! Exports are written as mboxrd, which quotes every line that could be
//! mistaken for a `From ` line with `>` and ends each message with an empty
//! line. [`Reader`] also reads what other mail apps write: Thunderbird's
//! `From - ` lines, messages without the empty line, CRLF line endings, and
//! the dates Google Takeout adds a zone to.

use std::collections::HashSet;
use std::io::{self, BufRead};

/// The `From ` line that starts a message.
pub(crate) fn separator(sender: &str, date_ms: i64) -> String {
    let date =
        chrono::DateTime::from_timestamp_millis(date_ms).unwrap_or(chrono::DateTime::UNIX_EPOCH);
    let sender =
        if sender.is_empty() || sender.contains(|c: char| c.is_whitespace() || c.is_control()) {
            "MAILER-DAEMON"
        } else {
            sender
        };
    format!("From {sender} {}\n", date.format("%a %b %e %H:%M:%S %Y"))
}

/// A message as it follows its `From ` line: LF line endings, `>`-quoted
/// `From ` lines, and the empty line that ends it.
pub(crate) fn entry(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 64);
    for line in message.split_inclusive(|&byte| byte == b'\n') {
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let unquoted = content.iter().position(|&byte| byte != b'>');
        if unquoted.is_some_and(|start| content[start..].starts_with(b"From ")) {
            out.push(b'>');
        }
        out.extend_from_slice(content);
        out.push(b'\n');
    }
    out.push(b'\n');
    out
}

/// A file name for a folder's mbox file that every platform accepts and no
/// other folder of the export already uses, whatever the letter case.
pub(crate) fn file_name(folder: &str, taken: &mut HashSet<String>) -> String {
    let cleaned: String = folder
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect();
    let base = match cleaned.trim().trim_matches('.') {
        "" => "Folder",
        base => base,
    };
    let mut name = format!("{base}.mbox");
    let mut number = 2;
    while !taken.insert(name.to_lowercase()) {
        name = format!("{base} ({number}).mbox");
        number += 1;
    }
    name
}

/// One entry of an mbox file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Message {
        /// The message without its `From ` line, with quoting removed.
        raw: Vec<u8>,
        /// When the message was stored, from its `From ` line.
        stored_at: Option<i64>,
    },
    /// A message larger than the reader's limit. It was skipped.
    TooLarge,
}

/// Reads the messages of an mbox file one at a time, holding at most one
/// message in memory.
pub(crate) struct Reader<R> {
    input: R,
    max_message_bytes: usize,
    /// The `From ` line that starts the next message.
    next: Option<Vec<u8>>,
    /// Whether `From ` lines carry a date. Mail that has one is only split
    /// at `From ` lines that have one too, since a message body can hold a
    /// line that merely starts with "From ". A file whose first `From ` line
    /// has no date is split at `From ` lines that follow an empty line.
    dated: bool,
    started: bool,
    position: u64,
    line: Vec<u8>,
}

impl<R: BufRead> Reader<R> {
    pub(crate) fn new(input: R, max_message_bytes: usize) -> Self {
        Self {
            input,
            max_message_bytes,
            next: None,
            dated: true,
            started: false,
            position: 0,
            line: Vec::new(),
        }
    }

    /// How many bytes of the file have been read.
    pub(crate) fn position(&self) -> u64 {
        self.position
    }

    /// The next message, or `None` at the end of the file. Fails with
    /// [`io::ErrorKind::InvalidData`] when the file isn't an mbox file.
    pub(crate) fn next_entry(&mut self) -> io::Result<Option<Entry>> {
        if !self.started {
            self.started = true;
            self.start()?;
        }
        let Some(separator) = self.next.take() else {
            return Ok(None);
        };
        let stored_at = from_line_date(&separator);
        let mut raw = Vec::new();
        let mut too_large = false;
        let mut after_empty_line = false;
        loop {
            let length = self.read_line()?;
            if length == 0 {
                break;
            }
            if self.line.starts_with(b"From ") && self.separates(after_empty_line) {
                self.next = Some(std::mem::take(&mut self.line));
                break;
            }
            after_empty_line = matches!(self.line.as_slice(), b"\n" | b"\r\n");
            if too_large {
                continue;
            }
            let line = unquote(&self.line);
            if length > self.line.len() || raw.len() + line.len() > self.max_message_bytes {
                too_large = true;
                raw = Vec::new();
                continue;
            }
            raw.extend_from_slice(line);
        }
        if too_large {
            return Ok(Some(Entry::TooLarge));
        }
        // The empty line that ends a message belongs to the file.
        if raw.ends_with(b"\r\n\r\n") {
            raw.truncate(raw.len() - 2);
        } else if raw.ends_with(b"\n\n") {
            raw.pop();
        }
        Ok(Some(Entry::Message { raw, stored_at }))
    }

    /// Finds the first `From ` line. Empty lines before it are tolerated.
    fn start(&mut self) -> io::Result<()> {
        loop {
            if self.read_line()? == 0 {
                return Ok(());
            }
            if matches!(self.line.as_slice(), b"\n" | b"\r\n") {
                continue;
            }
            if !self.line.starts_with(b"From ") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "not an mbox file",
                ));
            }
            self.dated = from_line_date(&self.line).is_some();
            self.next = Some(std::mem::take(&mut self.line));
            return Ok(());
        }
    }

    /// Whether the `From ` line just read starts a new message.
    fn separates(&self, after_empty_line: bool) -> bool {
        if self.dated {
            from_line_date(&self.line).is_some()
        } else {
            after_empty_line
        }
    }

    /// Reads one line, its line break included, into `self.line`, keeping at
    /// most one byte more than a message may hold. Returns the line's full
    /// length, which is 0 at the end of the file.
    fn read_line(&mut self) -> io::Result<usize> {
        self.line.clear();
        let keep = self.max_message_bytes + 1;
        let mut length = 0;
        loop {
            let available = match self.input.fill_buf() {
                Ok(available) => available,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                break;
            }
            let (used, ended) = match available.iter().position(|&byte| byte == b'\n') {
                Some(end) => (end + 1, true),
                None => (available.len(), false),
            };
            let room = keep.saturating_sub(self.line.len());
            self.line.extend_from_slice(&available[..used.min(room)]);
            self.input.consume(used);
            length += used;
            if ended {
                break;
            }
        }
        self.position += length as u64;
        Ok(length)
    }
}

/// Removes the quoting mboxrd adds to lines that start with "From " after
/// any number of `>`.
fn unquote(line: &[u8]) -> &[u8] {
    let quotes = line.iter().take_while(|&&byte| byte == b'>').count();
    if quotes > 0 && line[quotes..].starts_with(b"From ") {
        &line[1..]
    } else {
        line
    }
}

/// The date of a `From ` line in milliseconds. The line names the sender,
/// which may be missing or `-`, and then the time in ctime form, such as
/// `From alice@example.com Thu Jan  1 00:00:00 1970`. Some writers leave out
/// the seconds or add a zone before or after the year.
pub(crate) fn from_line_date(line: &[u8]) -> Option<i64> {
    let text = String::from_utf8_lossy(line.strip_prefix(b"From ")?);
    let tokens = text.split_ascii_whitespace().collect::<Vec<_>>();
    (0..tokens.len()).find_map(|start| ctime(&tokens[start..]))
}

fn ctime(tokens: &[&str]) -> Option<i64> {
    const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let [weekday, month, day, time, rest @ ..] = tokens else {
        return None;
    };
    let name = |token: &str| token.get(..3).map(str::to_ascii_lowercase);
    if !WEEKDAYS.contains(&name(weekday)?.as_str()) {
        return None;
    }
    let month = name(month)?;
    let month = (1..).zip(MONTHS).find(|(_, known)| *known == month)?.0;
    let day = day.parse::<u32>().ok()?;
    let mut clock = time.split(':').map(|part| part.parse::<u32>().ok());
    let (hour, minute) = (clock.next()??, clock.next()??);
    let second = clock.next().unwrap_or(Some(0))?;
    if clock.next().is_some() {
        return None;
    }
    let mut year = None;
    let mut offset_minutes = 0;
    for token in rest.iter().take(2) {
        if token.len() == 4 && token.bytes().all(|byte| byte.is_ascii_digit()) {
            year = token.parse::<i32>().ok();
        } else if let Some(minutes) = zone_offset(token) {
            offset_minutes = minutes;
        }
    }
    let local = chrono::NaiveDate::from_ymd_opt(year?, month, day)?
        .and_hms_opt(hour, minute, second)?
        .and_utc()
        .timestamp_millis();
    Some(local - i64::from(offset_minutes) * 60_000)
}

/// A numeric zone such as `+0200`, in minutes east of UTC.
fn zone_offset(token: &str) -> Option<i32> {
    let (sign, digits) = match token.as_bytes().first()? {
        b'+' => (1, &token[1..]),
        b'-' => (-1, &token[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = digits.parse::<i32>().ok()?;
    Some(sign * (value / 100 * 60 + value % 100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_quoted_and_separated() {
        assert_eq!(
            separator("alice@example.com", 0),
            "From alice@example.com Thu Jan  1 00:00:00 1970\n"
        );
        assert!(separator("", 0).starts_with("From MAILER-DAEMON "));
        assert!(separator("two words", 0).starts_with("From MAILER-DAEMON "));
        assert_eq!(
            entry(b"Subject: x\r\n\r\nFrom here\r\n>From there\r\n>>From far\r\n>\r\nlast"),
            b"Subject: x\n\n>From here\n>>From there\n>>>From far\n>\nlast\n\n"
        );
        assert_eq!(entry(b"From:me\n"), b"From:me\n\n");
    }

    fn read_all(file: &[u8]) -> io::Result<Vec<Entry>> {
        let mut reader = Reader::new(file, 1024);
        let mut entries = Vec::new();
        while let Some(entry) = reader.next_entry()? {
            entries.push(entry);
        }
        assert_eq!(reader.position(), file.len() as u64);
        Ok(entries)
    }

    fn message(raw: &[u8], stored_at: Option<i64>) -> Entry {
        Entry::Message {
            raw: raw.to_vec(),
            stored_at,
        }
    }

    #[test]
    fn exports_read_back_unchanged() {
        let originals: [&[u8]; 3] = [
            b"Subject: one\n\nFrom here\n>From there\n",
            b"Subject: two\n\nno line break at the end",
            b"Subject: three\n\n\n",
        ];
        let mut file = Vec::new();
        for original in originals {
            file.extend_from_slice(separator("alice@example.com", 86_400_000).as_bytes());
            file.extend(entry(original));
        }
        assert_eq!(
            read_all(&file).unwrap(),
            [
                message(originals[0], Some(86_400_000)),
                message(
                    b"Subject: two\n\nno line break at the end\n",
                    Some(86_400_000)
                ),
                message(originals[2], Some(86_400_000)),
            ]
        );
    }

    #[test]
    fn dated_mail_is_split_only_at_dated_from_lines() {
        let file = b"From - Mon Jan  1 00:00:00 2001\r\nSubject: a\r\n\r\n\
            From me to you\r\n\r\nFrom - Tue Jan  2 00:00:00 2001\r\nSubject: b\r\n\r\nb\r\n\
            From - Wed Jan  3 00:00:00 2001\r\nSubject: c\r\n\r\nc\r\n";
        assert_eq!(
            read_all(file).unwrap(),
            [
                message(
                    b"Subject: a\r\n\r\nFrom me to you\r\n",
                    Some(978_307_200_000)
                ),
                message(b"Subject: b\r\n\r\nb\r\n", Some(978_393_600_000)),
                message(b"Subject: c\r\n\r\nc\r\n", Some(978_480_000_000)),
            ]
        );
    }

    #[test]
    fn undated_mail_is_split_after_empty_lines() {
        let file = b"\nFrom alice\nSubject: a\n\nbody\nFrom here\n\nFrom bob\nSubject: b\n\nb\n";
        assert_eq!(
            read_all(file).unwrap(),
            [
                message(b"Subject: a\n\nbody\nFrom here\n", None),
                message(b"Subject: b\n\nb\n", None),
            ]
        );
    }

    #[test]
    fn oversized_messages_are_skipped() {
        let mut file = b"From - Mon Jan  1 00:00:00 2001\nSubject: long line\n\n".to_vec();
        file.extend([b'x'; 2000]);
        file.extend(b"\n\nFrom - Mon Jan  1 00:00:00 2001\nSubject: many lines\n\n");
        for _ in 0..200 {
            file.extend(b"0123456789\n");
        }
        file.extend(b"\nFrom - Mon Jan  1 00:00:00 2001\nSubject: small\n\nok\n");
        assert_eq!(
            read_all(&file).unwrap(),
            [
                Entry::TooLarge,
                Entry::TooLarge,
                message(b"Subject: small\n\nok\n", Some(978_307_200_000)),
            ]
        );
    }

    #[test]
    fn other_files_are_not_mbox_files() {
        let error = read_all(b"Subject: hello\n\nFrom - Mon Jan  1 00:00:00 2001\n").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(read_all(b"").unwrap(), []);
        assert_eq!(read_all(b"\r\n\n").unwrap(), []);
    }

    #[test]
    fn from_line_dates_allow_zones_and_missing_seconds() {
        let date = |line: &[u8]| from_line_date(line);
        assert_eq!(
            date(b"From 1797@xxx Wed Jan 01 00:00:00 +0000 2020\n"),
            Some(1_577_836_800_000)
        );
        assert_eq!(
            date(b"From - Thu Jan  1 01:00:00 1970\r\n"),
            Some(3_600_000)
        );
        assert_eq!(date(b"From x Thu Jan  1 02:00 +0200 1970\n"), Some(0));
        assert_eq!(
            date(b"From x Thu Jan  1 00:00:00 1970 -0100\n"),
            Some(3_600_000)
        );
        assert_eq!(date(b"From Thu Jan  1 00:00:00 1970\n"), Some(0));
        assert_eq!(date(b"From my experience, it works\n"), None);
        assert_eq!(date(b"From x Thu Jan 32 00:00:00 1970\n"), None);
        assert_eq!(date(b"From x Thu Jan  1 00:00:00:00 1970\n"), None);
        assert_eq!(date(b"Subject: Thu Jan  1 00:00:00 1970\n"), None);
    }

    #[test]
    fn folder_file_names_are_safe_and_unique() {
        let mut taken = HashSet::new();
        assert_eq!(file_name("Inbox", &mut taken), "Inbox.mbox");
        assert_eq!(file_name("inbox", &mut taken), "inbox (2).mbox");
        assert_eq!(file_name("Work/Q: 1?", &mut taken), "Work_Q_ 1_.mbox");
        assert_eq!(file_name("..", &mut taken), "Folder.mbox");
        assert_eq!(file_name(" \u{7} ", &mut taken), "_.mbox");
    }
}
