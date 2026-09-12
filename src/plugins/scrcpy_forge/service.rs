//! ScrcpyForge HTTP adapter independent from GTK.

use super::config::{Endpoints, PageConfig};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io::Read,
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
struct CachedPreview {
    etag: Option<String>,
    bytes: bytes::Bytes,
    last_used: Instant,
}

#[derive(Default)]
struct PreviewCache {
    entries: HashMap<String, CachedPreview>,
    total_bytes: usize,
}

#[derive(Clone)]
pub struct Client {
    api_url: String,
    endpoints: Endpoints,
    http: reqwest::Client,
    metadata_ttl: Duration,
    metadata: Arc<tokio::sync::Mutex<Option<CachedMetadata>>>,
    previews: Arc<tokio::sync::Mutex<PreviewCache>>,
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
const MAX_METADATA_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
enum ServiceError {
    #[error("{endpoint} transport error ({kind})")]
    Transport {
        endpoint: String,
        kind: &'static str,
    },
    #[error("{endpoint} returned HTTP {status}")]
    Status { endpoint: String, status: u16 },
    #[error("{endpoint} response exceeded {limit} bytes")]
    TooLarge { endpoint: String, limit: usize },
    #[error("{endpoint} response decode failed: {detail}")]
    Decode { endpoint: String, detail: String },
    #[error("{endpoint} returned invalid payload: {detail}")]
    InvalidPayload { endpoint: String, detail: String },
}

fn transport_error(endpoint: &str, error: &reqwest::Error) -> anyhow::Error {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else {
        "request"
    };
    anyhow::Error::new(ServiceError::Transport {
        endpoint: endpoint.to_owned(),
        kind,
    })
}

fn checked_response(
    response: reqwest::Response,
    endpoint: &str,
) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow::Error::new(ServiceError::Status {
            endpoint: endpoint.to_owned(),
            status: status.as_u16(),
        }));
    }
    Ok(response)
}

async fn send_checked(
    request: reqwest::RequestBuilder,
    endpoint: &str,
) -> anyhow::Result<reqwest::Response> {
    let response = request
        .send()
        .await
        .map_err(|error| transport_error(endpoint, &error))?;
    checked_response(response, endpoint)
}

async fn json_limited<T: DeserializeOwned>(
    response: reqwest::Response,
    endpoint: &str,
    limit: usize,
) -> anyhow::Result<T> {
    let bytes = response_bytes_limited(response, endpoint, limit).await?;
    serde_json::from_slice(&bytes).map_err(|error| {
        anyhow::Error::new(ServiceError::Decode {
            endpoint: endpoint.to_owned(),
            detail: error.to_string(),
        })
    })
}

async fn response_bytes_limited(
    mut response: reqwest::Response,
    endpoint: &str,
    limit: usize,
) -> anyhow::Result<bytes::Bytes> {
    let limit = limit.max(1);
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(anyhow::Error::new(ServiceError::TooLarge {
            endpoint: endpoint.to_owned(),
            limit,
        }));
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .map(|length| length as usize)
            .unwrap_or(64 * 1024)
            .min(limit),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| transport_error(endpoint, &error))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(anyhow::Error::new(ServiceError::TooLarge {
                endpoint: endpoint.to_owned(),
                limit,
            }));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(bytes))
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
            previews: Arc::new(tokio::sync::Mutex::new(PreviewCache::default())),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.api_url, path.trim_start_matches('/'))
    }
    fn endpoint(&self, template: &str, serial: &str) -> String {
        template.replace("{serial}", &encode_path_segment(serial))
    }
    pub async fn healthy(&self) -> bool {
        crate::core::power_debug::increment(crate::core::power_debug::Counter::HttpRequest);
        send_checked(self.http.get(self.url(&self.endpoints.health)), "health")
            .await
            .is_ok()
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        send_checked(
            self.http
                .post(self.url(&self.endpoints.shutdown))
                .timeout(Duration::from_secs(2)),
            "shutdown",
        )
        .await?;
        Ok(())
    }
    pub async fn connect(&self, endpoint: &str) -> anyhow::Result<()> {
        send_checked(
            self.http
                .post(self.url(&self.endpoints.connect))
                .json(&serde_json::json!({"endpoint":endpoint})),
            "connect",
        )
        .await?;
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
                let preview_endpoint = if has_session {
                    "session_preview"
                } else {
                    "device_preview"
                };
                let png = match request.send().await {
                    Ok(response) if response.status() == reqwest::StatusCode::NOT_MODIFIED => {
                        cached.map(|value| value.bytes)
                    }
                    Ok(response) => match checked_response(response, preview_endpoint) {
                        Ok(response) => {
                            let etag = response
                                .headers()
                                .get(reqwest::header::ETAG)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_owned);
                            match response_bytes_limited(
                                response,
                                preview_endpoint,
                                MAX_PREVIEW_BYTES,
                            )
                            .await
                            {
                                Ok(bytes) => {
                                    client.store_preview(cache_key, etag, bytes.clone()).await;
                                    Some(bytes)
                                }
                                Err(error) => {
                                    crate::core::error_limiter::warn(
                                        format!("scrcpy-forge:{preview_endpoint}"),
                                        error.to_string(),
                                    );
                                    None
                                }
                            }
                        }
                        Err(error) => {
                            crate::core::error_limiter::warn(
                                format!("scrcpy-forge:{preview_endpoint}"),
                                error.to_string(),
                            );
                            None
                        }
                    },
                    Err(error) => {
                        crate::core::error_limiter::warn(
                            format!("scrcpy-forge:{preview_endpoint}"),
                            transport_error(preview_endpoint, &error).to_string(),
                        );
                        None
                    }
                };
                let metrics =
                    if has_session {
                        crate::core::power_debug::increment(
                            crate::core::power_debug::Counter::HttpRequest,
                        );
                        match send_checked(
                            client.http.get(client.url(
                                &client.endpoint(&client.endpoints.session_metrics, &d.serial),
                            )),
                            "session_metrics",
                        )
                        .await
                        {
                            Ok(response) => match json_limited::<SessionMetrics>(
                                response,
                                "session_metrics",
                                MAX_METADATA_BYTES,
                            )
                            .await
                            {
                                Ok(metrics) => match validate_session_metrics(&metrics) {
                                    Ok(()) => Some(metrics),
                                    Err(error) => {
                                        crate::core::error_limiter::warn(
                                            "scrcpy-forge:session_metrics",
                                            error.to_string(),
                                        );
                                        None
                                    }
                                },
                                Err(error) => {
                                    crate::core::error_limiter::warn(
                                        "scrcpy-forge:session_metrics",
                                        error.to_string(),
                                    );
                                    None
                                }
                            },
                            Err(error) => {
                                crate::core::error_limiter::warn(
                                    "scrcpy-forge:session_metrics",
                                    error.to_string(),
                                );
                                None
                            }
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
        let devices_request =
            send_checked(self.http.get(self.url(&self.endpoints.devices)), "devices");
        let scripts_request = send_checked(self.http.get(self.url(&self.endpoints.tasks)), "tasks");
        let runs_request = send_checked(
            self.http.get(self.url(&self.endpoints.task_runs)),
            "task_runs",
        );
        let sessions_request = send_checked(
            self.http.get(self.url(&self.endpoints.sessions)),
            "sessions",
        );
        let (mut devices, mut scripts, mut runs, mut sessions) = tokio::try_join!(
            async {
                json_limited::<Vec<Device>>(devices_request.await?, "devices", MAX_METADATA_BYTES)
                    .await
            },
            async {
                json_limited::<Vec<String>>(scripts_request.await?, "tasks", MAX_METADATA_BYTES)
                    .await
            },
            async {
                json_limited::<Vec<ScriptRun>>(runs_request.await?, "task_runs", MAX_METADATA_BYTES)
                    .await
            },
            async {
                json_limited::<Vec<String>>(sessions_request.await?, "sessions", MAX_METADATA_BYTES)
                    .await
            },
        )?;
        validate_metadata(&devices, &scripts, &runs, &sessions)?;
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
        let mut cache = self.previews.lock().await;
        let preview = cache.entries.get_mut(key)?;
        preview.last_used = Instant::now();
        Some(preview.clone())
    }

    async fn store_preview(&self, key: String, etag: Option<String>, bytes: bytes::Bytes) {
        let mut cache = self.previews.lock().await;
        let byte_len = bytes.len();
        if let Some(previous) = cache.entries.insert(
            key,
            CachedPreview {
                etag,
                bytes,
                last_used: Instant::now(),
            },
        ) {
            cache.total_bytes = cache.total_bytes.saturating_sub(previous.bytes.len());
        }
        cache.total_bytes = cache.total_bytes.saturating_add(byte_len);
        while cache.entries.len() > MAX_PREVIEW_CACHE_ENTRIES
            || cache.total_bytes > MAX_PREVIEW_CACHE_BYTES
        {
            let Some(oldest_key) = cache
                .entries
                .iter()
                .min_by_key(|(_, preview)| preview.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(removed) = cache.entries.remove(&oldest_key) {
                cache.total_bytes = cache.total_bytes.saturating_sub(removed.bytes.len());
            }
        }
    }

    async fn prune_previews(&self, active_keys: HashSet<String>) {
        let mut cache = self.previews.lock().await;
        let mut removed_bytes = 0_usize;
        cache.entries.retain(|key, preview| {
            if active_keys.contains(key) {
                true
            } else {
                removed_bytes = removed_bytes.saturating_add(preview.bytes.len());
                false
            }
        });
        cache.total_bytes = cache.total_bytes.saturating_sub(removed_bytes);
    }

    pub async fn start_session(&self, serial: &str) -> anyhow::Result<()> {
        send_checked(
            self.http
                .post(self.url(&self.endpoint(&self.endpoints.session_start, serial)))
                .json(&serde_json::json!({})),
            "session_start",
        )
        .await?;
        self.invalidate_metadata().await;
        Ok(())
    }
    pub async fn run_script(&self, serial: &str, name: &str) -> anyhow::Result<()> {
        send_checked(
            self.http
                .post(self.url(&self.endpoints.task_run))
                .json(&RunNamed { serial, name }),
            "task_run",
        )
        .await?;
        self.invalidate_metadata().await;
        Ok(())
    }
    pub async fn stop_script(&self, serial: &str) -> anyhow::Result<()> {
        send_checked(
            self.http
                .post(self.url(&self.endpoint(&self.endpoints.task_stop, serial))),
            "task_stop",
        )
        .await?;
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
        send_checked(
            self.http
                .post(self.url(&path))
                .json(&serde_json::json!({"profile":profile})),
            kind,
        )
        .await?;
        Ok(())
    }
}

const MAX_SERVICE_DEVICES: usize = 128;
const MAX_SERVICE_SCRIPTS: usize = 256;
const MAX_SERVICE_RUNS: usize = 512;
const MAX_SERVICE_SESSIONS: usize = 128;
const MAX_SERVICE_TEXT_BYTES: usize = 512;

fn validate_metadata(
    devices: &[Device],
    scripts: &[String],
    runs: &[ScriptRun],
    sessions: &[String],
) -> anyhow::Result<()> {
    if devices.len() > MAX_SERVICE_DEVICES
        || scripts.len() > MAX_SERVICE_SCRIPTS
        || runs.len() > MAX_SERVICE_RUNS
        || sessions.len() > MAX_SERVICE_SESSIONS
    {
        return Err(anyhow::Error::new(ServiceError::InvalidPayload {
            endpoint: "metadata".into(),
            detail: format!(
                "collection sizes exceed limits ({MAX_SERVICE_DEVICES}/{MAX_SERVICE_SCRIPTS}/{MAX_SERVICE_RUNS}/{MAX_SERVICE_SESSIONS})"
            ),
        }));
    }
    for device in devices {
        validate_text_field("metadata.devices.serial", &device.serial)?;
        validate_text_field("metadata.devices.state", &device.state)?;
        if let Some(model) = &device.model {
            validate_text_field("metadata.devices.model", model)?;
        }
    }
    for script in scripts {
        validate_text_field("metadata.scripts", script)?;
    }
    for session in sessions {
        validate_text_field("metadata.sessions", session)?;
    }
    for run in runs {
        validate_text_field("metadata.runs.serial", &run.serial)?;
        if let Some(name) = &run.name {
            validate_text_field("metadata.runs.name", name)?;
        }
        if let Some(error) = &run.error {
            validate_text_field("metadata.runs.error", error)?;
        }
    }
    Ok(())
}

fn validate_text_field(field: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() || value.len() > MAX_SERVICE_TEXT_BYTES {
        return Err(anyhow::Error::new(ServiceError::InvalidPayload {
            endpoint: "metadata".into(),
            detail: format!("{field} is empty or exceeds {MAX_SERVICE_TEXT_BYTES} bytes"),
        }));
    }
    Ok(())
}

fn validate_session_metrics(metrics: &SessionMetrics) -> anyhow::Result<()> {
    let values = [
        metrics.decoded_fps,
        metrics.preview_fps,
        metrics.script_fps,
        metrics.latest_frame_age_ms,
        metrics.average_script_ms,
        metrics.script_p50_ms,
        metrics.script_p95_ms,
    ];
    if values.iter().any(|value| !value.is_finite())
        || metrics.profile.len() > MAX_SERVICE_TEXT_BYTES
        || metrics.preview_profile.len() > MAX_SERVICE_TEXT_BYTES
    {
        return Err(anyhow::Error::new(ServiceError::InvalidPayload {
            endpoint: "session_metrics".into(),
            detail: "metrics contain a non-finite value or oversized profile".into(),
        }));
    }
    Ok(())
}

fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

#[derive(Serialize)]
struct RunNamed<'a> {
    serial: &'a str,
    name: &'a str,
}

#[derive(Clone, Default)]
pub struct DaemonController {
    child: Rc<RefCell<Option<Child>>>,
    program: Rc<RefCell<Option<String>>>,
}

const MAX_DAEMON_STDERR_TAIL_BYTES: usize = 64 * 1024;

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
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        if let Some(stderr) = child.stderr.take() {
            let program_name = program.to_owned();
            let _ = std::thread::Builder::new()
                .name("pulsedeck-daemon-stderr".into())
                .spawn(move || drain_daemon_stderr(stderr, &program_name));
        }
        *self.program.borrow_mut() = Some(program.to_owned());
        *self.child.borrow_mut() = Some(child);
        Ok(())
    }
    pub fn stop(&self) {
        if let Some(child) = self.take() {
            // Process reaping and the grace period must not block the GTK
            // main loop. The caller can use `take` when it needs to sequence
            // the service API shutdown before this worker.
            let _ = std::thread::Builder::new()
                .name("pulsedeck-daemon-stop".into())
                .spawn(|| stop_child_blocking(child));
        }
    }

    pub fn take(&self) -> Option<Child> {
        self.program.borrow_mut().take();
        self.child.borrow_mut().take()
    }
    pub fn running(&self) -> bool {
        let mut slot = self.child.borrow_mut();
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    let program = self
                        .program
                        .borrow_mut()
                        .take()
                        .unwrap_or_else(|| "scrcpy-forge-daemon".into());
                    tracing::warn!(
                        program = %program,
                        exit_code = ?status.code(),
                        "ScrcpyForge daemon exited before/while becoming healthy"
                    );
                    *slot = None;
                    false
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to inspect ScrcpyForge daemon status");
                    true
                }
            }
        } else {
            false
        }
    }
}

pub(super) fn stop_child_blocking(mut child: Child) {
    #[cfg(unix)]
    {
        // A caller normally attempted the HTTP shutdown first. Give a
        // cooperative daemon a short window to exit before sending a signal;
        // this also keeps the common path free of unnecessary process-group
        // termination.
        if wait_for_child_exit(&mut child, Duration::from_millis(500)) {
            return;
        }
        let process_group = -(child.id() as libc::pid_t);
        unsafe {
            let _ = libc::kill(process_group, libc::SIGTERM);
        }
        if wait_for_child_exit(&mut child, Duration::from_secs(2)) {
            return;
        }
        unsafe {
            let _ = libc::kill(process_group, libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(unix)]
fn wait_for_child_exit(child: &mut Child, grace: Duration) -> bool {
    let deadline = std::time::Instant::now()
        .checked_add(grace)
        .unwrap_or_else(std::time::Instant::now);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => return false,
        }
    }
}
impl Drop for DaemonController {
    fn drop(&mut self) {
        if Rc::strong_count(&self.child) == 1 {
            self.stop()
        }
    }
}

fn drain_daemon_stderr(mut stderr: impl Read, program: &str) {
    let mut tail = std::collections::VecDeque::with_capacity(MAX_DAEMON_STDERR_TAIL_BYTES);
    let mut buffer = [0_u8; 4096];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                for byte in &buffer[..read] {
                    if tail.len() == MAX_DAEMON_STDERR_TAIL_BYTES {
                        tail.pop_front();
                    }
                    tail.push_back(*byte);
                }
            }
            Err(error) => {
                tracing::debug!(%error, program, "failed to drain ScrcpyForge daemon stderr");
                break;
            }
        }
    }
    if tail.is_empty() {
        return;
    }
    let bytes = tail.into_iter().collect::<Vec<_>>();
    let message = String::from_utf8_lossy(&bytes);
    let message = message.trim();
    if !message.is_empty() {
        tracing::warn!(program, stderr_tail = %message, "ScrcpyForge daemon stderr");
    }
}
