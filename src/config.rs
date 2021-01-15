use anyhow::Result;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use tracing::{debug, info, info_span};

#[derive(Debug, Copy, Clone)]
pub enum ProxyProtocol {
    Enabled,
    Disabled,
}

impl Default for ProxyProtocol {
    fn default() -> Self {
        ProxyProtocol::Disabled
    }
}

impl<'de> Deserialize<'de> for ProxyProtocol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match bool::deserialize(deserializer)? {
            true => Ok(ProxyProtocol::Enabled),
            false => Ok(ProxyProtocol::Disabled),
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct Api {
    pub http: Option<String>,
    #[serde(default)]
    pub http_proxy: ProxyProtocol,
    pub https: Option<String>,
    #[serde(default)]
    pub https_proxy: ProxyProtocol,
    pub prom: Option<String>,
    #[serde(default)]
    pub prom_proxy: ProxyProtocol,
}

fn default_acme() -> String {
    "https://acme-v02.api.letsencrypt.org/directory".to_string()
}

#[derive(Deserialize, Debug)]
pub struct General {
    pub dns: String,
    pub db: String,
    #[serde(default = "default_acme")]
    pub acme: String,
    pub name: String,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub general: General,
    pub api: Api,
    #[serde(default)]
    pub records: HashMap<String, Vec<Vec<String>>>,
}

const DEFAULT_CONFIG_PATH: &str = "config.toml";

// is not async so we can use it to load settings for tokio runtime
pub fn load_config(config_path: Option<String>) -> Result<Config> {
    let config_path = config_path.as_deref().unwrap_or(DEFAULT_CONFIG_PATH);

    let span = info_span!("load_config", config_path);
    let _enter = span.enter();

    let mut file = File::open(config_path)?;
    debug!(?file, "Opened file");

    let mut bytes = vec![];
    file.read_to_end(&mut bytes)?;
    debug!(file_length = bytes.len(), "Read file");

    let config = toml::de::from_slice::<Config>(&bytes)?;
    // redact db information
    let config_str = format!("{:?}", config).replace(&config.general.db, "******");
    info!(config = %config_str, "Deserialized config");

    Ok(config)
}
