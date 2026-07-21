//! ScrcpyForge HTTP adapter independent from GTK.

use super::config::{Endpoints, PageConfig};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::HashMap,
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
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.api_url, path.trim_start_matches('/'))
    }
    fn endpoint(&self, template: &str, serial: &str) -> String {
        template.replace("{serial}", serial)
    }
    pub async fn healthy(&self) -> bool {
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
    pub async fn snapshot(&self) -> anyhow::Result<Snapshot> {
        let metadata = self.metadata().await?;
        let Metadata {
            devices,
            scripts,
            runs,
            sessions,
        } = metadata;
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
                let png = match client
                    .http
                    .get(client.url(&path))
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                {
                    Ok(r) => r.bytes().await.ok(),
                    Err(_) => None,
                };
                let metrics =
                    if has_session {
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
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        *self.0.borrow_mut() = Some(child);
        Ok(())
    }
    pub fn stop(&self) {
        if let Some(mut child) = self.0.borrow_mut().take() {
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
