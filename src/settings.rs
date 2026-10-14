use std::env;

use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Deserializer, de};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct VideoPath(PathBuf);

impl VideoPath {
    pub fn as_path(&self) -> &Path { &self.0 }
    pub fn to_str(&self) -> Option<&str> { self.as_path().to_str() }
}

impl<'de> Deserialize<'de> for VideoPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let p = PathBuf::from(&s);

        if !p.is_file() {
            return Err(de::Error::custom(format!("not a file: {s}")));
        }

        Ok(VideoPath(p))
    }
}


#[derive(Debug, Deserialize, Clone)]
#[allow(unused)]
pub struct Settings {
    pub playlist: Vec<Fragment>,
    pub idle: Fragment,
    pub rotation_deg: Option<i64>,
    pub http_port: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(unused)]
pub struct Fragment {
    pub name: String,
    pub intro: Option<VideoPath>,
    pub main: VideoPath,
    pub outro: Option<VideoPath>,
    pub looping: bool,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let config_name = env::var("CONFIG_FILE").unwrap_or_else(|_| "main".into());
        let s = Config::builder()
            .add_source(File::with_name(&format!("cfg/{config_name}")))
            .add_source(Environment::with_prefix("venevit"))
            .build()?;
        s.try_deserialize()
    }
}
