use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    pub(crate) image: Option<String>,
    pub(crate) port: Option<u16>,
    pub(crate) cpus: Option<u8>,
    pub(crate) memory: Option<String>,
    pub(crate) ttl: Option<String>,
    pub(crate) idle_timeout: Option<String>,
    pub(crate) max_cpus: Option<u8>,
    pub(crate) max_memory: Option<String>,
    #[serde(default)]
    pub(crate) tailscale: TailscaleConfig,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TailscaleConfig {
    pub(crate) tag: Option<String>,
    #[serde(rename = "authKeyEnv")]
    pub(crate) auth_key_env: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let config: Config =
            toml::from_str("image = 'alpine'\nport = 8080\ncpus = 2\nmemory = '2G'\n").unwrap();
        assert_eq!(config.image.as_deref(), Some("alpine"));
        assert_eq!(config.port, Some(8080));
    }

    #[test]
    fn parses_tailscale_table() {
        let config: Config =
            toml::from_str("[tailscale]\ntag = 'tag:custom'\nauthKeyEnv = 'MY_KEY'\n").unwrap();
        assert_eq!(config.tailscale.tag.as_deref(), Some("tag:custom"));
        assert_eq!(config.tailscale.auth_key_env.as_deref(), Some("MY_KEY"));
    }

    #[test]
    fn tailscale_table_is_optional() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.tailscale.tag.is_none());
        assert!(config.tailscale.auth_key_env.is_none());
    }
}
