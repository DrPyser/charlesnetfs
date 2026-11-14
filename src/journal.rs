use std::fmt::{Display, Formatter};
use std::io::{BufRead, BufReader, Read};
use ureq::OrAnyStatus;

pub struct JournalEntry {
    pub cursor: String,
    pub message: String,
    pub timestamp: u64,
}

impl Display for JournalEntry {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "[{}] {}", self.timestamp, self.message)?;
        Ok(())
    }
}

impl JournalEntry {
    fn from_json(entry: &serde_json::Map<String, serde_json::Value>) -> Option<Self> {
        let message = entry
            .get("MESSAGE")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();

        let timestamp = entry
            .get("_SOURCE_REALTIME_TIMESTAMP")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let cursor = entry
            .get("__CURSOR")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();

        Some(JournalEntry {
            cursor,
            message,
            timestamp,
        })
    }
}

fn parse_entry(line: &str) -> Option<JournalEntry> {
    let parsed = serde_json::from_str::<serde_json::Value>(line).ok()?;
    parsed.as_object().and_then(JournalEntry::from_json)
}

fn parse_response<R: Read>(body: R) -> Vec<JournalEntry> {
    BufReader::new(body)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.is_empty())
        .filter_map(|l| parse_entry(l.as_str()))
        .collect()
}

pub fn fetch_journal(target: &str, port: u16, limit: u64) -> Result<Vec<JournalEntry>, String> {
    let agent = ureq::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build();

    let url = format!("http://{}:{}/entries", target, port);
    let range = format!("entries=:-{}:", limit - 1);

    let response = agent
        .get(&url)
        .set("Range", &range)
        .set("Accept", "application/json")
        .call()
        .or_any_status()
        .map_err(|e| format!("Connection failed: {}", e))?;

    let status = response.status();
    if status != 200 {
        return Err(format!(
            "HTTP error: {}: {}",
            status,
            response.into_string().unwrap_or("[non-utf8 body]".into())
        ));
    }

    Ok(parse_response(response.into_reader()))
}

pub fn fetch_after_cursor(
    host: &str,
    port: u16,
    cursor: &str,
    timeout: std::time::Duration,
) -> Result<Vec<JournalEntry>, String> {
    let agent = ureq::builder().timeout(timeout).build();

    let url = format!("http://{}:{}/entries", host, port);
    let range = format!("entries={}", cursor);

    let response = agent
        .get(&url)
        .set("Range", &range)
        .set("Accept", "application/json")
        .call()
        .or_any_status()
        .map_err(|e| format!("Connection failed: {}", e))?;

    let status = response.status();
    if status != 200 {
        return Err(format!(
            "HTTP error: {}: {}",
            status,
            response.into_string().unwrap_or("[non-utf8 body]".into())
        ));
    }

    Ok(parse_response(response.into_reader()))
}

pub struct JournalStream {
    reader: BufReader<Box<dyn Read + Send + Sync>>,
}

impl Iterator for JournalStream {
    type Item = Result<JournalEntry, String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Err(e) => Some(Err(e.to_string())),
            Ok(0) => None, // EOF
            Ok(_) => {
                if let Some(entry) = parse_entry(&line) {
                    Some(Ok(entry))
                } else {
                    Some(Err(format!("failed to parse log entry: {}", line)))
                }
            }
        }
    }
}

pub fn follow_journal(host: &str, port: u16) -> Result<JournalStream, String> {
    let page = fetch_journal(host, port, 1)?;
    let cursor = page
        .last()
        .map(|entry| entry.cursor.clone())
        .unwrap_or_default();

    let agent = ureq::builder().build();
    let url = format!("http://{}:{}/entries", host, port);
    let range = format!("entries={}:0:", cursor);

    let response = agent
        .get(&url)
        .set("Range", &range)
        .set("Accept", "application/json")
        .query("follow", "")
        .call()
        .or_any_status()
        .map_err(|e| format!("Connection failed: {}", e))?;

    let status = response.status();
    if status != 200 {
        return Err(format!(
            "HTTP error: {}: {}",
            status,
            response.into_string().unwrap_or("[non-utf8 body]".into())
        ));
    }
    let reader = BufReader::new(response.into_reader());
    Ok(JournalStream { reader })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry_json(cursor: &str, message: &str, timestamp: u64) -> String {
        format!(
            r#"{{"__CURSOR":"{}","MESSAGE":"{}","_SOURCE_REALTIME_TIMESTAMP":"{}"}}"#,
            cursor, message, timestamp
        )
    }

    #[test]
    fn test_journal_entry_display() {
        let entry = JournalEntry {
            cursor: "s=c0e3...:t=123".to_string(),
            message: "systemd[1]: Started foo.service".to_string(),
            timestamp: 1700000000,
        };
        let output = format!("{}", entry);
        assert!(output.contains("systemd[1]: Started foo.service"));
    }

    #[test]
    fn test_from_json_parses_all_fields() {
        let json = serde_json::from_str::<serde_json::Value>(&sample_entry_json(
            "s=abc",
            "hello world",
            1700000000,
        ))
        .unwrap();
        let entry = JournalEntry::from_json(json.as_object().unwrap()).unwrap();
        assert_eq!(entry.cursor, "s=abc");
        assert_eq!(entry.message, "hello world");
        assert_eq!(entry.timestamp, 1700000000);
    }

    #[test]
    fn test_from_json_missing_fields_defaults() {
        let json = serde_json::from_str::<serde_json::Value>(r#"{"MESSAGE":"test"}"#).unwrap();
        let entry = JournalEntry::from_json(json.as_object().unwrap()).unwrap();
        assert_eq!(entry.message, "test");
        assert_eq!(entry.cursor, "");
        assert_eq!(entry.timestamp, 0);
    }

    #[test]
    fn test_from_json_empty_object() {
        let json = serde_json::from_str::<serde_json::Value>(r#"{}"#).unwrap();
        let entry = JournalEntry::from_json(json.as_object().unwrap()).unwrap();
        assert_eq!(entry.message, "");
        assert_eq!(entry.cursor, "");
        assert_eq!(entry.timestamp, 0);
    }

    #[test]
    fn test_parse_entry_valid() {
        let line = sample_entry_json("s=x", "hello", 42);
        let entry = parse_entry(&line).unwrap();
        assert_eq!(entry.cursor, "s=x");
        assert_eq!(entry.message, "hello");
        assert_eq!(entry.timestamp, 42);
    }

    #[test]
    fn test_parse_entry_invalid_json() {
        assert!(parse_entry("not json").is_none());
    }

    #[test]
    fn test_parse_entry_empty_string() {
        assert!(parse_entry("").is_none());
    }

    #[test]
    fn test_parse_response_multiple_lines() {
        let e1 = sample_entry_json("s=1", "one", 1);
        let e2 = sample_entry_json("s=2", "two", 2);
        let body = format!("{}\n{}\n", e1, e2).into_bytes();
        let entries = parse_response(body.as_slice());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].cursor, "s=1");
        assert_eq!(entries[1].cursor, "s=2");
    }

    #[test]
    fn test_parse_response_skips_empty_lines() {
        let e = sample_entry_json("s=1", "one", 1);
        let body = format!("\n{}\n\n{}\n", e, e).into_bytes();
        let entries = parse_response(body.as_slice());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_parse_response_empty_body() {
        assert!(parse_response("".as_bytes()).is_empty());
    }
}
