use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TargetConfig {
    pub addr: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
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
            addr: "192.168.1.1:22".to_string(),
            timeout_ms: None,
        };
        assert_eq!(target.timeout(5000), 5000);
    }

    #[test]
    fn test_target_config_timeout_uses_override_when_set() {
        let target = TargetConfig {
            addr: "192.168.1.1:22".to_string(),
            timeout_ms: Some(3000),
        };
        assert_eq!(target.timeout(5000), 3000);
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
        assert_eq!(router.addr, "192.168.1.1:22");
        assert_eq!(router.timeout(5000), 5000); // uses global default

        let laptop = &config.targets["laptop"];
        assert_eq!(laptop.timeout(5000), 3000); // overrides global
    }
}
