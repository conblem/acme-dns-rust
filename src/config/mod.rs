use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use tracing::{debug, info, info_span};

pub use listener::{Listener, ProxyProtocol};

mod listener;
mod records;

#[derive(Deserialize, Debug)]
pub struct Api {
    pub http: Listener,
    pub https: Listener,
    pub prom: Listener,
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
