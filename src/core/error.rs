use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("timeout")]
    Timeout,

    #[error("cancelled")]
    Cancelled,

    #[error("process error: {0}")]
    Process(String),

    #[error("http error: {0}")]
    Http(String),

    #[error("dbus error: {0}")]
    Dbus(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error("config parse error in {path}: {message}")]
    ConfigParse { path: PathBuf, message: String },

    #[error("secrets file has wrong permissions: {0}")]
    SecretsPermission(PathBuf),
}
