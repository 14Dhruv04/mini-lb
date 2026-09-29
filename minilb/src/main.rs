use axum::{

    body::{to_bytes, Body},

    extract::State,

    http::{Request, Response, StatusCode},

    routing::{any, get, post},

    Json, Router,

};

use bytes::Bytes;

use reqwest::Client;

use serde::{Deserialize, Serialize};

use serde_json::json;

use std::{

    net::SocketAddr,

    path::PathBuf,

    sync::Arc,

    time::{Duration, Instant, SystemTime, UNIX_EPOCH},

};

use tokio::sync::{mpsc, RwLock};

use tracing::{error, info, warn};



#[derive(Debug, Clone, Deserialize, Serialize)]

struct Config {

    listen: String,

    #[serde(default)]

    admin_listen: Option<String>,

    health_check_interval_seconds: Option<u64>,

    health_check_path: Option<String>,

    backends: Vec<BackendConfig>,

    control_plane: Option<ControlPlaneConfig>,

    telemetry: Option<TelemetryConfig>,

}



#[derive(Debug, Clone, Deserialize, Serialize)]

struct ControlPlaneConfig {

    url: String,

    token: String,

    #[serde(default = "default_control_plane_poll_seconds")]

    poll_interval_seconds: u64,

}



fn default_control_plane_poll_seconds() -> u64 {

    10

}



#[derive(Debug, Clone, Deserialize, Serialize)]

struct TelemetryConfig {

    url: String,

    token: String,

    #[serde(default = "default_true")]

    enabled: bool,

}



#[derive(Debug, Clone, Serialize)]

struct TelemetryEvent {

    event_type: String,

    timestamp_ms: u64,

    request_id: Option<String>,

    method: Option<String>,

    path: Option<String>,

    backend: Option<String>,

    status: Option<u16>,

    latency_ms: Option<u64>,

    healthy: Option<bool>,

}



#[derive(Debug, Clone, Deserialize, Serialize)]

struct RemoteConfig {

    instance_id: String,

    version: u64,

    health_check_interval_seconds: Option<u64>,

    health_check_path: Option<String>,

    backends: Vec<BackendConfig>,

}



#[derive(Debug, Clone, Deserialize, Serialize)]

struct BackendConfig {

    id: String,

    url: String,

    #[serde(default = "default_true")]

    enabled: bool,

}



fn default_true() -> bool {

    true

}



#[derive(Debug, Clone, Serialize)]

struct BackendStatus {

    id: String,

    url: String,

    enabled: bool,

    healthy: bool,

}



#[derive(Debug, Clone, Serialize)]

struct StatusResponse {

    strategy: &'static str,

    config_path: String,

    health_check_path: String,

    health_check_interval_seconds: u64,

    backends: Vec<BackendStatus>,

}



#[derive(Debug, Clone)]

struct Backend {

    id: String,

    url: String,

    enabled: bool,

    healthy: bool,

}



#[derive(Clone)]

struct AppState {

    client: Client,

    backends: Arc<RwLock<Vec<Backend>>>,

    next_index: Arc<RwLock<usize>>,

    health_check_path: Arc<RwLock<String>>,

    health_check_interval: Arc<RwLock<Duration>>,

    config_path: Arc<PathBuf>,

    control_plane: Option<ControlPlaneConfig>,

    remote_version: Arc<RwLock<u64>>,

    telemetry_tx: Option<mpsc::Sender<TelemetryEvent>>,

}



#[tokio::main]

async fn main() {

    tracing_subscriber::fmt().with_env_filter("info").init();



    let config_path = PathBuf::from(

        std::env::var("MINILB_CONFIG").unwrap_or_else(|_| "minilb.yml".to_string()),

    );

    let config = load_config(&config_path).await;



    // Client used for backend proxying, health checks, and control-plane polling.
    // Keep its timeout relatively tight so application traffic is never held up
    // by a slow remote control plane.
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("failed to create HTTP client");

    // Telemetry is best-effort and runs in a background worker, so give it
    // a longer request timeout without changing the timeout for the data plane
    // or control-plane requests.
    let telemetry_client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to create telemetry HTTP client");

    let telemetry_tx = config.telemetry.clone()
        .filter(|cfg| cfg.enabled)
        .map(|cfg| {
            let (tx, rx) = mpsc::channel(1024);

            tokio::spawn(async move {
                run_telemetry_worker(telemetry_client, cfg, rx).await;
            });

            tx
        });



    let state = AppState {

        client: client.clone(),

        backends: Arc::new(RwLock::new(build_backends(&config.backends, None))),

        next_index: Arc::new(RwLock::new(0)),

        health_check_path: Arc::new(RwLock::new(

            config

                .health_check_path

                .clone()

                .unwrap_or_else(|| "/health".to_string()),

        )),

        health_check_interval: Arc::new(RwLock::new(Duration::from_secs(

            config.health_check_interval_seconds.unwrap_or(5),

        ))),

        config_path: Arc::new(config_path.clone()),

        control_plane: config.control_plane.clone(),

        remote_version: Arc::new(RwLock::new(0)),

        telemetry_tx,

    };



    let health_state = state.clone();

    tokio::spawn(async move {

        run_health_checks(health_state).await;

    });



    let watcher_state = state.clone();

    tokio::spawn(async move {

        watch_config(watcher_state).await;

    });



    if state.control_plane.is_some() {

        let remote_state = state.clone();

        tokio::spawn(async move {

            watch_remote_config(remote_state).await;

        });

    }



    let proxy_app = Router::new()

        .route("/{*path}", any(proxy))

        .with_state(state.clone());



    let addr: SocketAddr = config.listen.parse().expect("invalid listen address");

    info!(config = %config_path.display(), "loaded MiniLB configuration");

    info!("MiniLB listening on http://{}", addr);



    let listener = tokio::net::TcpListener::bind(addr)

        .await

        .expect("bind failed");

    tokio::spawn(async move {

        if let Err(err) = axum::serve(listener, proxy_app).await {

            error!(error = %err, "proxy server failed");

        }

    });



    let admin_listen = config

        .admin_listen

        .unwrap_or_else(|| "0.0.0.0:9090".to_string());

    let admin_addr: SocketAddr = admin_listen

        .parse()

        .expect("invalid admin_listen address");

    let admin_app = Router::new()

        .route("/status", get(status))

        .route("/reload", post(reload))

        .with_state(state);



    info!("MiniLB admin API listening on http://{}", admin_addr);

    let admin_listener = tokio::net::TcpListener::bind(admin_addr)

        .await

        .expect("admin bind failed");

    if let Err(err) = axum::serve(admin_listener, admin_app).await {

        error!(error = %err, "admin server failed");

    }

}



async fn load_config(path: &PathBuf) -> Config {

    let config_text = tokio::fs::read_to_string(path)

        .await

        .unwrap_or_else(|err| panic!("failed to read config {}: {err}", path.display()));

    serde_yaml::from_str(&config_text)

        .unwrap_or_else(|err| panic!("invalid config {}: {err}", path.display()))

}



fn build_backends(

    configs: &[BackendConfig],

    previous: Option<&[Backend]>,

) -> Vec<Backend> {

    configs

        .iter()

        .map(|cfg| {

            let healthy = previous

                .and_then(|old| old.iter().find(|b| b.id == cfg.id && b.url == cfg.url))

                .map(|b| b.healthy)

                .unwrap_or(false);



            Backend {

                id: cfg.id.clone(),

                url: cfg.url.clone(),

                enabled: cfg.enabled,

                healthy,

            }

        })

        .collect()

}



async fn reload_from_disk(state: &AppState) -> Result<(), String> {

    let config = load_config(state.config_path.as_ref()).await;

    let previous = state.backends.read().await.clone();



    {

        let mut backends = state.backends.write().await;

        *backends = build_backends(&config.backends, Some(&previous));

    }



    {

        let mut path = state.health_check_path.write().await;

        *path = config

            .health_check_path

            .clone()

            .unwrap_or_else(|| "/health".to_string());

    }



    {

        let mut interval = state.health_check_interval.write().await;

        *interval = Duration::from_secs(config.health_check_interval_seconds.unwrap_or(5));

    }



    *state.next_index.write().await = 0;

    info!("reloaded MiniLB backend configuration");

    Ok(())

}



async fn status(State(state): State<AppState>) -> Json<StatusResponse> {

    let backends = state.backends.read().await;

    let path = state.health_check_path.read().await.clone();

    let interval = state.health_check_interval.read().await.as_secs();



    Json(StatusResponse {

        strategy: "round_robin",

        config_path: state.config_path.display().to_string(),

        health_check_path: path,

        health_check_interval_seconds: interval,

        backends: backends

            .iter()

            .map(|b| BackendStatus {

                id: b.id.clone(),

                url: b.url.clone(),

                enabled: b.enabled,

                healthy: b.healthy,

            })

            .collect(),

    })

}



async fn reload(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {

    match reload_from_disk(&state).await {

        Ok(()) => (

            StatusCode::OK,

            Json(json!({"status": "reloaded"})),

        ),

        Err(err) => (

            StatusCode::INTERNAL_SERVER_ERROR,

            Json(json!({"status": "error", "message": err})),

        ),

    }

}



async fn proxy(State(state): State<AppState>, req: Request<Body>) -> Response<Body> {

    let started = Instant::now();

    let request_id = uuid::Uuid::new_v4().to_string();

    let method_name = req.method().as_str().to_string();

    let path = req.uri().path().to_string();



    let backend = match choose_backend(&state).await {

        Some(b) => b,

        None => {

            emit_event(&state, TelemetryEvent {

                event_type: "request".to_string(),

                timestamp_ms: now_ms(),

                request_id: Some(request_id),

                method: Some(method_name),

                path: Some(path),

                backend: None,

                status: Some(StatusCode::SERVICE_UNAVAILABLE.as_u16()),

                latency_ms: Some(started.elapsed().as_millis() as u64),

                healthy: None,

            });

            return Response::builder()

                .status(StatusCode::SERVICE_UNAVAILABLE)

                .body(Body::from("no healthy backends available"))

                .unwrap();

        }

    };



    let path_and_query = req

        .uri()

        .path_and_query()

        .map(|x| x.as_str())

        .unwrap_or("/");

    let target = format!("{}{}", backend.url.trim_end_matches('/'), path_and_query);

    let method = req.method().clone();

    let headers = req.headers().clone();

    let body = match to_bytes(req.into_body(), 8 * 1024 * 1024).await {

        Ok(b) => b,

        Err(_) => {

            emit_event(&state, TelemetryEvent {

                event_type: "request".to_string(),

                timestamp_ms: now_ms(),

                request_id: Some(request_id),

                method: Some(method_name),

                path: Some(path),

                backend: Some(backend.id),

                status: Some(StatusCode::BAD_REQUEST.as_u16()),

                latency_ms: Some(started.elapsed().as_millis() as u64),

                healthy: None,

            });

            return simple_error(StatusCode::BAD_REQUEST, "unable to read request body");

        }

    };



    let mut builder = state.client.request(method, &target);

    for (name, value) in &headers {

        if name.as_str().eq_ignore_ascii_case("host") || is_hop_by_hop(name.as_str()) {

            continue;

        }

        builder = builder.header(name, value);

    }



    match builder.body(body).send().await {

        Ok(upstream) => {

            let status = upstream.status();

            let upstream_headers = upstream.headers().clone();

            let bytes = upstream.bytes().await.unwrap_or_else(|_| Bytes::new());

            let latency_ms = started.elapsed().as_millis() as u64;

            emit_event(&state, TelemetryEvent {

                event_type: "request".to_string(),

                timestamp_ms: now_ms(),

                request_id: Some(request_id),

                method: Some(method_name),

                path: Some(path),

                backend: Some(backend.id.clone()),

                status: Some(status.as_u16()),

                latency_ms: Some(latency_ms),

                healthy: None,

            });

            let mut response = Response::builder().status(status);

            if let Some(headers_mut) = response.headers_mut() {

                for (name, value) in &upstream_headers {

                    if is_hop_by_hop(name.as_str()) {

                        continue;

                    }

                    headers_mut.insert(name, value.clone());

                }

            }

            response.body(Body::from(bytes)).unwrap()

        }

        Err(err) => {

            error!(backend = %backend.id, error = %err, "upstream request failed");

            emit_event(&state, TelemetryEvent {

                event_type: "request".to_string(),

                timestamp_ms: now_ms(),

                request_id: Some(request_id),

                method: Some(method_name),

                path: Some(path),

                backend: Some(backend.id),

                status: Some(StatusCode::BAD_GATEWAY.as_u16()),

                latency_ms: Some(started.elapsed().as_millis() as u64),

                healthy: None,

            });

            simple_error(StatusCode::BAD_GATEWAY, "upstream request failed")

        }

    }

}



fn now_ms() -> u64 {

    SystemTime::now()

        .duration_since(UNIX_EPOCH)

        .unwrap_or_default()

        .as_millis() as u64

}



fn emit_event(state: &AppState, event: TelemetryEvent) {

    if let Some(tx) = &state.telemetry_tx {

        if tx.try_send(event).is_err() {

            warn!("telemetry queue is full or unavailable; dropping event");

        }

    }

}



async fn run_telemetry_worker(
    client: Client,
    config: TelemetryConfig,
    mut rx: mpsc::Receiver<TelemetryEvent>,
) {
    info!(endpoint = %config.url, "MiniLB telemetry enabled");

    while let Some(event) = rx.recv().await {
        match client
            .post(config.url.trim_end_matches('/'))
            .header("x-minilb-token", &config.token)
            .json(&event)
            .send()
            .await
        {
            Ok(response)
                if response.status().is_success()
                    || response.status() == StatusCode::ACCEPTED => {}

            Ok(response) => {
                warn!(
                    status = %response.status(),
                    "telemetry endpoint returned an error"
                );
            }

            Err(err) => {
                warn!(
                    error = %err,
                    is_timeout = err.is_timeout(),
                    is_connect = err.is_connect(),
                    "unable to send telemetry event"
                );
            }
        }
    }
}

fn simple_error(status: StatusCode, message: &'static str) -> Response<Body> {

    Response::builder()

        .status(status)

        .body(Body::from(message))

        .unwrap()

}



async fn choose_backend(state: &AppState) -> Option<Backend> {

    let backends = state.backends.read().await;

    if backends.is_empty() {

        return None;

    }



    let len = backends.len();

    let mut idx = state.next_index.write().await;

    for _ in 0..len {

        let candidate = backends[*idx % len].clone();

        *idx = (*idx + 1) % len;

        if candidate.enabled && candidate.healthy {

            return Some(candidate);

        }

    }

    None

}



async fn run_health_checks(state: AppState) {

    loop {

        let interval = *state.health_check_interval.read().await;

        let health_path = state.health_check_path.read().await.clone();

        let snapshot = state.backends.read().await.clone();



        for backend in snapshot {

            if !backend.enabled {

                continue;

            }



            let url = format!(

                "{}{}",

                backend.url.trim_end_matches('/'),

                health_path.as_str()

            );

            let healthy = match state.client.get(&url).send().await {

                Ok(r) => r.status().is_success(),

                Err(_) => false,

            };



            let mut backends = state.backends.write().await;

            if let Some(current) = backends.iter_mut().find(|b| b.id == backend.id) {

                if current.healthy != healthy {

                    info!(backend = %current.id, healthy, "backend health changed");

                    emit_event(&state, TelemetryEvent {

                        event_type: "health_change".to_string(),

                        timestamp_ms: now_ms(),

                        request_id: None,

                        method: None,

                        path: None,

                        backend: Some(current.id.clone()),

                        status: None,

                        latency_ms: None,

                        healthy: Some(healthy),

                    });

                }

                current.healthy = healthy;

            }

        }



        tokio::time::sleep(interval).await;

    }

}



async fn watch_config(state: AppState) {

    let mut last_modified: Option<SystemTime> = None;



    loop {

        match tokio::fs::metadata(state.config_path.as_ref()).await {

            Ok(metadata) => match metadata.modified() {

                Ok(modified) if last_modified != Some(modified) => {

                    if last_modified.is_some() {

                        match reload_from_disk(&state).await {

                            Ok(()) => info!("configuration file changed; applied new configuration"),

                            Err(err) => warn!(error = %err, "configuration file changed but reload failed"),

                        }

                    }

                    last_modified = Some(modified);

                }

                _ => {}

            },

            Err(err) => warn!(error = %err, "unable to stat configuration file"),

        }



        tokio::time::sleep(Duration::from_secs(1)).await;

    }

}



async fn apply_remote_config(state: &AppState, config: RemoteConfig) -> Result<bool, String> {

    let mut current_version = state.remote_version.write().await;

    if config.version <= *current_version {

        return Ok(false);

    }



    let previous = state.backends.read().await.clone();

    {

        let mut backends = state.backends.write().await;

        *backends = build_backends(&config.backends, Some(&previous));

    }



    {

        let mut path = state.health_check_path.write().await;

        *path = config

            .health_check_path

            .clone()

            .unwrap_or_else(|| "/health".to_string());

    }



    {

        let mut interval = state.health_check_interval.write().await;

        *interval = Duration::from_secs(config.health_check_interval_seconds.unwrap_or(5));

    }



    *state.next_index.write().await = 0;

    *current_version = config.version;

    info!(instance_id = %config.instance_id, version = config.version, "applied remote configuration");

    Ok(true)

}



async fn watch_remote_config(state: AppState) {

    let Some(control_plane) = state.control_plane.clone() else {

        return;

    };



    let endpoint = control_plane.url.trim_end_matches('/').to_string();

    let interval = Duration::from_secs(control_plane.poll_interval_seconds.max(1));



    loop {

        match state

            .client

            .get(&endpoint)

            .header("x-minilb-token", &control_plane.token)

            .send()

            .await

        {

            Ok(response) if response.status().is_success() => {

                match response.json::<RemoteConfig>().await {

                    Ok(config) => {

                        if let Err(err) = apply_remote_config(&state, config).await {

                            warn!(error = %err, "failed to apply remote configuration");

                        }

                    }

                    Err(err) => warn!(error = %err, "invalid remote configuration response"),

                }

            }

            Ok(response) => {

                warn!(status = %response.status(), "control plane returned an error");

            }

            Err(err) => warn!(error = %err, "unable to reach control plane"),

        }



        tokio::time::sleep(interval).await;

    }

}



fn is_hop_by_hop(name: &str) -> bool {

    matches!(

        name.to_ascii_lowercase().as_str(),

        "connection"

            | "keep-alive"

            | "proxy-authenticate"

            | "proxy-authorization"

            | "te"

            | "trailer"

            | "transfer-encoding"

            | "upgrade"

    )

}
