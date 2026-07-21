use std::collections::HashMap;
use std::fs;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use crate::core::config::secrets_path;
use crate::core::error::AppError;

#[derive(Debug, Clone)]
struct CacheEntry {
    body: String,
    timestamp: Instant,
}

pub struct HttpClientState {
    client: reqwest::Client,
    rt: tokio::runtime::Runtime,
    secrets: HashMap<String, String>,
    cache: RwLock<HashMap<String, CacheEntry>>,
}

impl HttpClientState {
    fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime for http client");

        Self {
            client,
            rt,
            secrets: load_secrets(),
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn get(
        &self,
        url: &str,
        headers: Option<&HashMap<String, String>>,
        timeout: Duration,
        cache_ttl: Option<Duration>,
    ) -> Result<String, AppError> {
        let url = self.substitute_secrets(url);

        if let Some(ttl) = cache_ttl {
            let cache = self.cache.read().unwrap();
            if let Some(entry) = cache.get(&url) {
                if entry.timestamp.elapsed() < ttl {
                    return Ok(entry.body.clone());
                }
            }
        }

        let mut req = self.client.get(&url).timeout(timeout);

        if let Some(hdrs) = headers {
            for (k, v) in hdrs {
                let v = self.substitute_secrets(v);
                req = req.header(k.as_str(), &v);
            }
        }

        let resp = self
            .rt
            .block_on(req.send())
            .map_err(|e| AppError::Http(e.to_string()))?;

        let body = self
            .rt
            .block_on(resp.text())
            .map_err(|e| AppError::Http(e.to_string()))?;

        if cache_ttl.is_some() {
            self.cache.write().unwrap().insert(
                url,
                CacheEntry {
                    body: body.clone(),
                    timestamp: Instant::now(),
                },
            );
        }

        Ok(body)
    }

    pub fn post(
        &self,
        url: &str,
        body: &str,
        headers: Option<&HashMap<String, String>>,
        timeout: Duration,
    ) -> Result<String, AppError> {
        let url = self.substitute_secrets(url);
        let body = self.substitute_secrets(body);

        let mut req = self.client.post(&url).body(body).timeout(timeout);

        if let Some(hdrs) = headers {
            for (k, v) in hdrs {
                let v = self.substitute_secrets(v);
                req = req.header(k.as_str(), &v);
            }
        }

        let resp = self
            .rt
            .block_on(req.send())
            .map_err(|e| AppError::Http(e.to_string()))?;

        let text = self
            .rt
            .block_on(resp.text())
            .map_err(|e| AppError::Http(e.to_string()))?;

        Ok(text)
    }

    pub fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        headers: Option<&HashMap<String, String>>,
        timeout: Duration,
        cache_ttl: Option<Duration>,
    ) -> Result<T, AppError> {
        let body = self.get(url, headers, timeout, cache_ttl)?;
        serde_json::from_str(&body).map_err(|e| AppError::Http(format!("json parse: {}", e)))
    }

    pub fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &str,
        headers: Option<&HashMap<String, String>>,
        timeout: Duration,
    ) -> Result<T, AppError> {
        let text = self.post(url, body, headers, timeout)?;
        serde_json::from_str(&text).map_err(|e| AppError::Http(format!("json parse: {}", e)))
    }

    fn substitute_secrets(&self, input: &str) -> String {
        let mut result = input.to_string();
        for (key, value) in &self.secrets {
            let placeholder = format!("{{secret:{}}}", key);
            result = result.replace(&placeholder, value);
        }
        result
    }
}

pub static HTTP_CLIENT: LazyLock<HttpClientState> = LazyLock::new(HttpClientState::new);

fn load_secrets() -> HashMap<String, String> {
    let path = secrets_path();
    match fs::read_to_string(&path) {
        Ok(content) => toml::from_str::<HashMap<String, String>>(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}
