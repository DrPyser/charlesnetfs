use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default = "default_timeout")]
    pub global_timeout_ms: u64,
    pub targets: BTreeMap<String, TargetConfig>,
}

fn default_timeout() -> u64 {
    5000
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            global_timeout_ms: default_timeout(),
            targets: BTreeMap::new(),
        }
    }
}

impl Default for TargetConfig {
    fn default() -> Self {
        Self {
            addr: Addr::new("localhost".to_string(), 0),
            timeout_ms: None,
        }
    }
}

impl TargetConfig {
    pub fn timeout(&self, global: u64) -> u64 {
        self.timeout_ms.unwrap_or(global)
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
targets:
  router:
    addr: 192.168.1.1:22
  nas:
    addr: 192.168.1.100:5000
  laptop:
    addr: 10.0.0.5:22
    timeout_ms: 3000
"#;
        let config = Config::load(yaml.as_bytes()).unwrap();
        assert_eq!(config.global_timeout_ms, 5000);
        assert!(config.targets.contains_key("router"));
        assert!(config.targets.contains_key("nas"));

        let router = &config.targets["router"];
        assert_eq!(router.addr.to_string(), "192.168.1.1:22");
        assert_eq!(router.addr.host(), "192.168.1.1");
        assert_eq!(router.addr.port(), 22);
        assert_eq!(router.timeout(5000), 5000);

        let laptop = &config.targets["laptop"];
        assert_eq!(laptop.timeout(5000), 3000); // overrides global
    }
}
