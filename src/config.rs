use serde::Deserialize;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use std::error::Error;

#[derive(Deserialize, Debug)]
pub struct Api {
    pub ip: String,
    pub port: String,
}

#[derive(Deserialize, Debug)]
pub struct General {
    pub listen: String
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub general: General,
    pub api: Api,
}


pub async fn config() -> Result<Config, Box<dyn Error>>{
    let mut file = File::open("config.toml").await?;
    let mut bytes = vec![];
    file.read_to_end(&mut bytes).await?;

    Ok(toml::de::from_slice::<Config>(&bytes)?)
}