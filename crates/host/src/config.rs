use std::{fs, net::SocketAddr, path::{Path, PathBuf}};
use serde::Deserialize;
use surveillance_detect::Region;
use crate::Result;

pub const WIDTH: usize = 160;
pub const HEIGHT: usize = 120;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub camera_enabled: bool,
    pub microphone_enabled: bool,
    pub camera_device: Option<String>,
    pub microphone_device: Option<String>,
    pub motion_region: [usize; 4],
    pub motion_threshold: f32,
    pub sound_margin_db: f32,
    pub retention_days: u64,
    pub firebase_credentials_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8765".parse().unwrap(),
            camera_enabled: true,
            microphone_enabled: true,
            camera_device: None,
            microphone_device: None,
            motion_region: [0, 0, WIDTH, HEIGHT],
            motion_threshold: 0.04,
            sound_margin_db: 14.0,
            retention_days: 7,
            firebase_credentials_file: std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").map(PathBuf::from),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = match fs::read_to_string(path) {
            Ok(contents) => toml::from_str(&contents)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error.into()),
        };
        if !config.listen.ip().is_loopback() {
            return Err("listen must be a loopback address; use an HTTPS reverse proxy".into());
        }
        if !(0.001..=0.5).contains(&config.motion_threshold) || config.region().is_none() {
            return Err("invalid motion threshold or region".into());
        }
        if !(3.0..=60.0).contains(&config.sound_margin_db) || !(1..=365).contains(&config.retention_days) {
            return Err("invalid sound margin or retention period".into());
        }
        Ok(config)
    }

    pub fn region(&self) -> Option<Region> {
        Region::new(WIDTH, HEIGHT, self.motion_region)
    }
}

pub fn data_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("org", "Surveillance", "Surveillance")
        .map(|project| project.data_local_dir().to_path_buf())
        .ok_or_else(|| "cannot resolve the local app data directory; use --data-dir".into())
}

pub fn create_data_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
