use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

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
    pub fn load(path: &Path) -> Result<Self, String> {
        let contents =
            fs::read_to_string(path).map_err(|e| format!("failed to read config: {}", e))?;

        serde_yaml::from_str(&contents).map_err(|e| format!("failed to parse config: {}", e))
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
        let config = Config::load(Path::new("config.yaml")).unwrap();
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
