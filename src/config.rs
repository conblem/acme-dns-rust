use serde::Deserialize;
use std::error::Error;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

#[derive(Deserialize, Debug)]
pub struct Api {
    pub http: Option<String>,
    pub https: Option<String>,
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
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub general: General,
    pub api: Api,
}

const DEFAULT_CONFIG_PATH: &str = "config.toml";

pub async fn config(config_path: Option<String>) -> Result<Config, Box<dyn Error>> {
    let config_path = config_path.as_deref().unwrap_or(DEFAULT_CONFIG_PATH);
    let mut file = File::open(config_path).await?;
    let mut bytes = vec![];
    file.read_to_end(&mut bytes).await?;

    Ok(toml::de::from_slice::<Config>(&bytes)?)
}
