use std::fmt::{Display, Formatter};

use ureq::{Agent, OrAnyStatus};

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


pub fn fetch_journal(target: &str, port: u16, limit: u64) -> Result<Vec<JournalEntry>, String> {
    let agent = Agent::new();

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

    // Response is newline-separated JSON objects
    let body = response
        .into_string()
        .map_err(|e| format!("failed to read response: {}", e))?;

    let entries: Vec<JournalEntry> = body
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let parsed = serde_json::from_str::<serde_json::Value>(line).ok()?;
            parsed.as_object().and_then(JournalEntry::from_json)
        })
        .collect();

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
