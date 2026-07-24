use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[cfg_attr(
        not(any(feature = "pet-card", feature = "scrcpy-forge")),
        allow(dead_code)
    )]
    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error("config parse error in {path}: {message}")]
    ConfigParse { path: PathBuf, message: String },
}
