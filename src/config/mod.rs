use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs::read;
use tracing::{debug, info, info_span, trace};

pub use listener::{Listener, ProxyProtocol};
pub use records::PreconfiguredRecords;
use trust_dns_server::resolver::config::ResolverConfig;

mod dns;
mod listener;
mod records;

#[derive(Deserialize, Debug)]
pub struct Api {
    #[serde(default, deserialize_with = "listener::deserialize")]
    pub http: Listener,
    #[serde(default, deserialize_with = "listener::deserialize")]
    pub https: Listener,
    #[serde(default, deserialize_with = "listener::deserialize")]
    pub prom: Listener,
}

const DEFAULT_ACME: &str = "https://acme-v02.api.letsencrypt.org/directory";
fn default_acme() -> String {
    DEFAULT_ACME.to_string()
}

#[derive(Deserialize, Debug)]
pub struct General {
    #[serde(default, deserialize_with = "dns::deserialize")]
    pub test: Option<ResolverConfig>,
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
    #[serde(default, deserialize_with = "records::deserialize")]
    pub records: PreconfiguredRecords,
}

const DEFAULT_CONFIG_PATH: &str = "config.toml";

// is not async so we can use it to load settings for tokio runtime
pub fn load_config(config_path: Option<String>) -> Result<Config> {
    let choose_default = config_path.is_none();
    let config_path = config_path.as_deref().unwrap_or(DEFAULT_CONFIG_PATH);

    let span = info_span!("load_config", config_path, choose_default);
    let _enter = span.enter();

    trace!("Start reading config file");
    let file = read(config_path).with_context(|| format!("{{config_path={}}}", config_path))?;
    debug!(file_length = file.len(), "Read file");

    trace!("Start deserializing config file");
    let config = toml::de::from_slice::<Config>(&file)?;
    // redact db information
    let config_str = format!("{:?}", config).replace(&config.general.db, "******");
    info!(config = %config_str, "Deserialized config");

    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use tracing_test::traced_test;

    use super::{default_acme, load_config, DEFAULT_ACME};

    #[test]
    #[traced_test]
    fn load_config_test() {
        let path = Path::new(file!()).with_file_name("test_config.toml");
        let path = path.to_string_lossy().into_owned();
        let config = load_config(Some(path)).unwrap();

        // check if logs contain redacted db information
        let config = format!("{:?}", config);
        let redacted_config = format!("{:?}", config).replace("postgres://root@localhost/acme", "******");
        // make sure redaction worked
        assert_eq!(config.len() > redacted_config.len());
        assert!(logs_contain(&config));
    }

    #[test]
    fn test_default_acme() {
        assert_eq!(DEFAULT_ACME, default_acme());
    }
}
