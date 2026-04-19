use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default = "default_timeout")]
    pub global_timeout_ms: u64,
    #[serde(default = "default_journal_limit")]
    pub journal_limit: u64,
    pub targets: BTreeMap<String, TargetConfig>,
}

const DEFAULT_TIMEOUT_MS: u64 = 5000;
const DEFAULT_JOURNAL_LIMIT: u64 = 50;
const DEFAULT_JOURNAL_PORT: u16 = 19531;

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn default_journal_limit() -> u64 {
    DEFAULT_JOURNAL_LIMIT
}

fn default_journal_port() -> u16 {
    DEFAULT_JOURNAL_PORT
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    host: String,
    port: u16,
}

impl Addr {
    pub fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let (host, port_str) = s
            .rsplit_once(':')
            .ok_or_else(|| format!("invalid addr '{}': expected host:port", s))?;
        if host.is_empty() {
            return Err(format!("invalid addr '{}': empty host", s));
        }
        let port: u16 = port_str
            .parse()
            .map_err(|_| format!("invalid addr '{}': invalid port", s))?;
        Ok(Self {
            host: host.to_string(),
            port,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl Serialize for Addr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Addr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TargetConfig {
    pub addr: Addr,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub journal_limit: Option<u64>,
    #[serde(default)]
    pub journal_port: Option<u16>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            global_timeout_ms: default_timeout(),
            journal_limit: default_journal_limit(),
            targets: BTreeMap::new(),
        }
    }
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self {
            addr: Addr::new("localhost".to_string(), 0),
            timeout_ms: None,
            journal_limit: None,
            journal_port: None,
        }
    }
}

impl TargetConfig {
    pub fn timeout(&self, global: u64) -> u64 {
        self.timeout_ms.unwrap_or(global)
    }

    pub fn journal_limit(&self, global: u64) -> u64 {
        self.journal_limit.unwrap_or(global)
    }

    pub fn journal_port(&self) -> u16 {
        self.journal_port.unwrap_or(default_journal_port())
    }
}

impl Config {
    pub fn load<R: Read>(reader: R) -> Result<Self, String> {
        serde_yaml::from_reader(reader).map_err(|e| format!("failed to parse config: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_config_timeout_uses_global_when_not_set() {
        let target = TargetConfig {
            addr: Addr::parse("192.168.1.1:22").unwrap(),
            ..Default::default()
        };
        assert_eq!(target.timeout(5000), 5000);
    }

    #[test]
    fn test_target_config_timeout_uses_override_when_set() {
        let target = TargetConfig {
            addr: Addr::parse("192.168.1.1:22").unwrap(),
            timeout_ms: Some(3000),
            ..Default::default()
        };
        assert_eq!(target.timeout(5000), 3000);
    }

    #[test]
    fn test_journal_limit_uses_global_when_not_set() {
        let target = TargetConfig {
            addr: Addr::parse("192.168.1.1:22").unwrap(),
            ..Default::default()
        };
        assert_eq!(target.journal_limit(50), 50);
    }

    #[test]
    fn test_journal_limit_uses_override_when_set() {
        let target = TargetConfig {
            addr: Addr::parse("192.168.1.1:22").unwrap(),
            journal_limit: Some(100),
            ..Default::default()
        };
        assert_eq!(target.journal_limit(50), 100);
    }

    #[test]
    fn test_journal_port_defaults_to_19531() {
        let target = TargetConfig {
            addr: Addr::parse("192.168.1.1:22").unwrap(),
            ..Default::default()
        };
        assert_eq!(target.journal_port(), 19531);
    }

    #[test]
    fn test_journal_port_uses_override_when_set() {
        let target = TargetConfig {
            addr: Addr::parse("192.168.1.1:22").unwrap(),
            journal_port: Some(8080),
            ..Default::default()
        };
        assert_eq!(target.journal_port(), 8080);
    }

    #[test]
    fn test_addr_parse_valid() {
        let addr = Addr::parse("192.168.1.1:22").unwrap();
        assert_eq!(addr.host(), "192.168.1.1");
        assert_eq!(addr.port(), 22);
        assert_eq!(addr.to_string(), "192.168.1.1:22");
    }

    #[test]
    fn test_addr_parse_no_port() {
        assert!(Addr::parse("192.168.1.1").is_err());
    }

    #[test]
    fn test_addr_parse_empty_host() {
        assert!(Addr::parse(":22").is_err());
    }

    #[test]
    fn test_addr_parse_invalid_port() {
        assert!(Addr::parse("192.168.1.1:abc").is_err());
    }

    #[test]
    fn test_config_load() {
        let yaml = r#"
global_timeout_ms: 5000
journal_limit: 50
targets:
  router:
    addr: 192.168.1.1:22
  nas:
    addr: 192.168.1.100:5000
  laptop:
    addr: 10.0.0.5:22
    timeout_ms: 3000
    journal_limit: 100
    journal_port: 19531
"#;
        let config = Config::load(yaml.as_bytes()).unwrap();
        assert_eq!(config.global_timeout_ms, 5000);
        assert_eq!(config.journal_limit, 50);
        assert!(config.targets.contains_key("router"));
        assert!(config.targets.contains_key("nas"));

        let router = &config.targets["router"];
        assert_eq!(router.addr.to_string(), "192.168.1.1:22");
        assert_eq!(router.addr.host(), "192.168.1.1");
        assert_eq!(router.addr.port(), 22);
        assert_eq!(router.timeout(5000), 5000);
        assert_eq!(router.journal_limit(50), 50);
        assert_eq!(router.journal_port(), 19531);

        let laptop = &config.targets["laptop"];
        assert_eq!(laptop.timeout(5000), 3000);
        assert_eq!(laptop.journal_limit(50), 100);
        assert_eq!(laptop.journal_port(), 19531);
    }

    #[test]
    fn test_config_load_default_journal_limit() {
        let yaml = r#"
global_timeout_ms: 5000
targets:
  router:
    addr: 192.168.1.1:22
"#;
        let config = Config::load(yaml.as_bytes()).unwrap();
        assert_eq!(config.journal_limit, 50);
    }
}
