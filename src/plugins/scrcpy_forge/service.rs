//! ScrcpyForge HTTP adapter independent from GTK.

use super::config::{Endpoints, PageConfig};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ScriptRun {
    pub serial: String,
    pub name: Option<String>,
    pub running: bool,
    #[serde(default)]
    pub stalled: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SessionMetrics {
    pub decoded_fps: f64,
    pub preview_fps: f64,
    pub script_fps: f64,
    #[serde(default)]
    pub latest_frame_age_ms: f64,
    pub average_script_ms: f64,
    pub script_p50_ms: f64,
    pub script_p95_ms: f64,
    pub dropped_script_frames: u64,
    pub profile: String,
    pub preview_profile: String,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub devices: Vec<(Device, Option<bytes::Bytes>)>,
    pub scripts: Vec<String>,
    pub runs: Vec<ScriptRun>,
    pub sessions: Vec<String>,
    pub metrics: HashMap<String, SessionMetrics>,
}
#[derive(Clone)]
struct Metadata {
    devices: Vec<Device>,
    scripts: Vec<String>,
    runs: Vec<ScriptRun>,
    sessions: Vec<String>,
}
struct CachedMetadata {
    fetched_at: Instant,
    value: Metadata,
}

#[derive(Clone)]
pub struct Client {
    api_url: String,
    endpoints: Endpoints,
    http: reqwest::Client,
    metadata_ttl: Duration,
    metadata: Arc<tokio::sync::Mutex<Option<CachedMetadata>>>,
    previews: Arc<tokio::sync::Mutex<HashMap<String, CachedPreview>>>,
}
#[derive(Clone)]
struct CachedPreview {
    etag: Option<String>,
    bytes: bytes::Bytes,
    last_used: Instant,
}

fn preview_cache_key(serial: &str, has_session: bool) -> String {
    format!(
        "{serial}:{}",
        if has_session { "session" } else { "device" }
    )
}

const MAX_PREVIEW_CACHE_ENTRIES: usize = 32;
const MAX_PREVIEW_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;

async fn response_bytes_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Option<bytes::Bytes> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return None;
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .map(|length| length as usize)
            .unwrap_or(64 * 1024)
            .min(limit),
    );
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes::Bytes::from(bytes))
}
impl Client {
    pub fn new(config: &PageConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(4))
            .build()
            .unwrap_or_default();
        Self {
            api_url: config.api_url.trim_end_matches('/').into(),
            endpoints: config.endpoints.clone(),
            http,
            metadata_ttl: Duration::from_secs(config.metadata_interval_seconds.max(1)),
            metadata: Arc::new(tokio::sync::Mutex::new(None)),
            previews: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.api_url, path.trim_start_matches('/'))
    }
    fn endpoint(&self, template: &str, serial: &str) -> String {
        template.replace("{serial}", serial)
    }
    pub async fn healthy(&self) -> bool {
        crate::core::power_debug::increment(crate::core::power_debug::Counter::HttpRequest);
        self.http
            .get(self.url(&self.endpoints.health))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.http
            .post(self.url(&self.endpoints.shutdown))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
    pub async fn connect(&self, endpoint: &str) -> anyhow::Result<()> {
        self.http
            .post(self.url(&self.endpoints.connect))
            .json(&serde_json::json!({"endpoint":endpoint}))
            .send()
            .await?
            .error_for_status()?;
        self.invalidate_metadata().await;
        Ok(())
    }
    pub async fn snapshot(&self, include_previews: bool) -> anyhow::Result<Snapshot> {
        let metadata = self.metadata().await?;
        let Metadata {
            devices,
            scripts,
            runs,
            sessions,
        } = metadata;
        if !include_previews {
            return Ok(Snapshot {
                devices: devices.into_iter().map(|device| (device, None)).collect(),
                scripts,
                runs,
                sessions,
                metrics: HashMap::new(),
            });
        }
        // Device screenshots and metrics are independent network operations. Fetch
        // them concurrently so multiple devices do not multiply the UI refresh delay.
        let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
        let mut tasks = tokio::task::JoinSet::new();
        for (index, d) in devices.into_iter().enumerate() {
            let has_session = d.state == "device" && sessions.contains(&d.serial);
            let client = self.clone();
            let semaphore = semaphore.clone();
            tasks.spawn(async move {
                let _permit = semaphore.acquire_owned().await.ok();
                let path = if has_session {
                    client.endpoint(&client.endpoints.session_preview, &d.serial)
                } else {
                    client.endpoint(&client.endpoints.device_preview, &d.serial)
                };
                let cache_key = preview_cache_key(&d.serial, has_session);
                crate::core::power_debug::increment(crate::core::power_debug::Counter::HttpRequest);
                let cached = client.cached_preview(&cache_key).await;
                let mut request = client.http.get(client.url(&path));
                if let Some(etag) = cached.as_ref().and_then(|value| value.etag.as_deref()) {
                    request = request.header(reqwest::header::IF_NONE_MATCH, etag);
                }
                let png = match request.send().await {
                    Ok(response) if response.status() == reqwest::StatusCode::NOT_MODIFIED => {
                        cached.map(|value| value.bytes)
                    }
                    Ok(response) => {
                        let response = response.error_for_status().ok();
                        if let Some(response) = response {
                            let etag = response
                                .headers()
                                .get(reqwest::header::ETAG)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_owned);
                            let bytes = response_bytes_limited(response, MAX_PREVIEW_BYTES).await;
                            if let Some(bytes) = bytes.as_ref() {
                                client.store_preview(cache_key, etag, bytes.clone()).await;
                            }
                            bytes
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };
                let metrics =
                    if has_session {
                        crate::core::power_debug::increment(
                            crate::core::power_debug::Counter::HttpRequest,
                        );
                        match client
                            .http
                            .get(client.url(
                                &client.endpoint(&client.endpoints.session_metrics, &d.serial),
                            ))
                            .send()
                            .await
                            .and_then(|r| r.error_for_status())
                        {
                            Ok(r) => r.json::<SessionMetrics>().await.ok(),
                            Err(_) => None,
                        }
                    } else {
                        None
                    };
                (index, d, png, metrics)
            });
        }
        let mut results = Vec::new();
        while let Some(Ok(value)) = tasks.join_next().await {
            results.push(value)
        }
        results.sort_by_key(|v| v.0);
        let mut previews = Vec::new();
        let mut metrics = HashMap::new();
        for (_, device, png, value) in results {
            if let Some(value) = value {
                metrics.insert(device.serial.clone(), value);
            }
            previews.push((device, png));
        }
        let active_keys = previews
            .iter()
            .map(|(device, _)| {
                preview_cache_key(
                    &device.serial,
                    device.state == "device" && sessions.contains(&device.serial),
                )
            })
            .collect();
        self.prune_previews(active_keys).await;
        Ok(Snapshot {
            devices: previews,
            scripts,
            runs,
            sessions,
            metrics,
        })
    }
    async fn metadata(&self) -> anyhow::Result<Metadata> {
        let mut cache = self.metadata.lock().await;
        if let Some(cached) = cache.as_ref() {
            if cached.fetched_at.elapsed() < self.metadata_ttl {
                return Ok(cached.value.clone());
            }
        }
        for _ in 0..4 {
            crate::core::power_debug::increment(crate::core::power_debug::Counter::HttpRequest);
        }
        // These resources do not depend on each other. Fetching them together
        // keeps refresh latency close to the slowest request instead of their sum.
        let devices_request = self.http.get(self.url(&self.endpoints.devices)).send();
        let scripts_request = self.http.get(self.url(&self.endpoints.tasks)).send();
        let runs_request = self.http.get(self.url(&self.endpoints.task_runs)).send();
        let sessions_request = self.http.get(self.url(&self.endpoints.sessions)).send();
        let (mut devices, mut scripts, mut runs, mut sessions) = tokio::try_join!(
            async {
                Ok::<_, anyhow::Error>(
                    devices_request
                        .await?
                        .error_for_status()?
                        .json::<Vec<Device>>()
                        .await?,
                )
            },
            async {
                Ok::<_, anyhow::Error>(
                    scripts_request
                        .await?
                        .error_for_status()?
                        .json::<Vec<String>>()
                        .await?,
                )
            },
            async {
                Ok::<_, anyhow::Error>(
                    runs_request
                        .await?
                        .error_for_status()?
                        .json::<Vec<ScriptRun>>()
                        .await?,
                )
            },
            async {
                Ok::<_, anyhow::Error>(
                    sessions_request
                        .await?
                        .error_for_status()?
                        .json::<Vec<String>>()
                        .await?,
                )
            },
        )?;
        devices.sort_by(|a, b| a.serial.cmp(&b.serial));
        scripts.sort();
        sessions.sort();
        runs.sort_by(|a, b| a.serial.cmp(&b.serial));
        let value = Metadata {
            devices,
            scripts,
            runs,
            sessions,
        };
        *cache = Some(CachedMetadata {
            fetched_at: Instant::now(),
            value: value.clone(),
        });
        Ok(value)
    }
    async fn invalidate_metadata(&self) {
        *self.metadata.lock().await = None;
    }

    async fn cached_preview(&self, key: &str) -> Option<CachedPreview> {
        let mut previews = self.previews.lock().await;
        let preview = previews.get_mut(key)?;
        preview.last_used = Instant::now();
        Some(preview.clone())
    }

    async fn store_preview(&self, key: String, etag: Option<String>, bytes: bytes::Bytes) {
        let mut previews = self.previews.lock().await;
        previews.insert(
            key,
            CachedPreview {
                etag,
                bytes,
                last_used: Instant::now(),
            },
        );
        while previews.len() > MAX_PREVIEW_CACHE_ENTRIES
            || previews
                .values()
                .map(|preview| preview.bytes.len())
                .sum::<usize>()
                > MAX_PREVIEW_CACHE_BYTES
        {
            let Some(oldest_key) = previews
                .iter()
                .min_by_key(|(_, preview)| preview.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            previews.remove(&oldest_key);
        }
    }

    async fn prune_previews(&self, active_keys: HashSet<String>) {
        self.previews
            .lock()
            .await
            .retain(|key, _| active_keys.contains(key));
    }

    pub async fn start_session(&self, serial: &str) -> anyhow::Result<()> {
        self.http
            .post(self.url(&self.endpoint(&self.endpoints.session_start, serial)))
            .json(&serde_json::json!({}))
            .send()
            .await?
            .error_for_status()?;
        self.invalidate_metadata().await;
        Ok(())
    }
    pub async fn run_script(&self, serial: &str, name: &str) -> anyhow::Result<()> {
        self.http
            .post(self.url(&self.endpoints.task_run))
            .json(&RunNamed { serial, name })
            .send()
            .await?
            .error_for_status()?;
        self.invalidate_metadata().await;
        Ok(())
    }
    pub async fn stop_script(&self, serial: &str) -> anyhow::Result<()> {
        self.http
            .post(self.url(&self.endpoint(&self.endpoints.task_stop, serial)))
            .send()
            .await?
            .error_for_status()?;
        self.invalidate_metadata().await;
        Ok(())
    }
    pub async fn set_script_profile(&self, serial: &str, profile: &str) -> anyhow::Result<()> {
        self.set_profile(serial, "script-profile", profile).await
    }
    pub async fn set_preview_profile(&self, serial: &str, profile: &str) -> anyhow::Result<()> {
        self.set_profile(serial, "preview-profile", profile).await
    }
    async fn set_profile(&self, serial: &str, kind: &str, profile: &str) -> anyhow::Result<()> {
        let path = self
            .endpoint(&self.endpoints.profile, serial)
            .replace("{kind}", kind);
        self.http
            .post(self.url(&path))
            .json(&serde_json::json!({"profile":profile}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}
#[derive(Serialize)]
struct RunNamed<'a> {
    serial: &'a str,
    name: &'a str,
}

#[derive(Clone, Default)]
pub struct DaemonController(Rc<RefCell<Option<Child>>>);
impl DaemonController {
    pub fn start(&self, program: &str, args: &[String]) -> std::io::Result<()> {
        if self.running() {
            return Ok(());
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn()?;
        *self.0.borrow_mut() = Some(child);
        Ok(())
    }
    pub fn stop(&self) {
        if let Some(mut child) = self.0.borrow_mut().take() {
            #[cfg(unix)]
            {
                let process_group = -(child.id() as libc::pid_t);
                unsafe {
                    let _ = libc::kill(process_group, libc::SIGTERM);
                    let _ = libc::kill(process_group, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    pub fn running(&self) -> bool {
        let mut slot = self.0.borrow_mut();
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(None) => true,
                _ => {
                    *slot = None;
                    false
                }
            }
        } else {
            false
        }
    }
}
impl Drop for DaemonController {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) == 1 {
            self.stop()
        }
    }
}
