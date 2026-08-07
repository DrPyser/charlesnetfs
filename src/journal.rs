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

fn send_entries<R: Read>(
    reader: R,
    chan: std::sync::mpsc::SyncSender<JournalEntry>,
) -> Result<(), String> {
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if let Some(entry) = parse_entry(&line) {
            chan.send(entry)
                .map_err(|_| "failed to push journal entry to main thread")?;
        }
    }
    Ok(())
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

pub fn follow_journal(
    host: &str,
    port: u16,
    chan: std::sync::mpsc::SyncSender<JournalEntry>,
) -> Result<(), String> {
    let cursor = fetch_journal(host, port, 1)?
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

    send_entries(response.into_reader(), chan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

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
        let body = format!("{}\n{}\n", e1, e2);
        let entries = parse_response(&body);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].cursor, "s=1");
        assert_eq!(entries[1].cursor, "s=2");
    }

    #[test]
    fn test_parse_response_skips_empty_lines() {
        let e = sample_entry_json("s=1", "one", 1);
        let body = format!("\n{}\n\n{}\n", e, e);
        let entries = parse_response(&body);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_parse_response_empty_body() {
        assert!(parse_response("").is_empty());
    }

    #[test]
    fn test_follow_journal_streams_entries() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            let body = sample_entry_json("s=first", "initial entry", 1000);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();

            let mut stream2 = listener.accept().unwrap().0;
            let entry1 = sample_entry_json("s=second", "entry two", 2000);
            let entry2 = sample_entry_json("s=third", "entry three", 3000);
            let body = format!("{}\n{}\n", entry1, entry2);
            let response = format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                body.len(),
                body
            );
            stream2.write_all(response.as_bytes()).unwrap();
            drop(stream2);
        });

        let (tx, rx) = mpsc::channel();
        let host = addr.ip().to_string();
        follow_journal(&host, addr.port(), tx).unwrap();

        let entries: Vec<JournalEntry> = rx.iter().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].cursor, "s=second");
        assert_eq!(entries[1].cursor, "s=third");
    }

    #[test]
    fn test_follow_journal_partial_line_buffering() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            let body = sample_entry_json("s=init", "init", 100);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();

            let mut stream2 = listener.accept().unwrap().0;
            let entry = sample_entry_json("s=buf", "buffered entry", 200);
            let mid = entry.len() / 2;
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", entry.len());
            stream2.write_all(resp.as_bytes()).unwrap();
            stream2.write_all(&entry.as_bytes()[..mid]).unwrap();
            thread::sleep(std::time::Duration::from_millis(50));
            stream2.write_all(&entry.as_bytes()[mid..]).unwrap();
            drop(stream2);
        });

        let (tx, rx) = mpsc::channel();
        let host = addr.ip().to_string();
        follow_journal(&host, addr.port(), tx).unwrap();

        let entries: Vec<JournalEntry> = rx.iter().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cursor, "s=buf");
        assert_eq!(entries[0].message, "buffered entry");
    }

    #[test]
    fn test_follow_journal_empty_cursor() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(resp.as_bytes()).unwrap();

            let mut stream2 = listener.accept().unwrap().0;
            let entry = sample_entry_json("s=first", "first entry", 500);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}\n",
                entry.len() + 1,
                entry
            );
            stream2.write_all(resp.as_bytes()).unwrap();
            drop(stream2);
        });

        let (tx, rx) = mpsc::channel();
        let host = addr.ip().to_string();
        follow_journal(&host, addr.port(), tx).unwrap();

        let entries: Vec<JournalEntry> = rx.iter().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cursor, "s=first");
    }

    #[test]
    fn test_follow_journal_http_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            let resp =
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 13\r\n\r\nserver error";
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let (tx, _rx) = mpsc::channel();
        let host = addr.ip().to_string();
        let result = follow_journal(&host, addr.port(), tx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("HTTP error"));
    }

    #[test]
    fn test_follow_journal_channel_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            let body = sample_entry_json("s=first", "init", 100);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();

            let mut stream2 = listener.accept().unwrap().0;
            let entry = sample_entry_json("s=lost", "no receiver", 200);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}\n",
                entry.len() + 1,
                entry
            );
            stream2.write_all(resp.as_bytes()).unwrap();
            drop(stream2);
        });

        let (tx, rx) = mpsc::channel();
        let host = addr.ip().to_string();
        drop(rx);
        let result = follow_journal(&host, addr.port(), tx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to push"));
    }
}
