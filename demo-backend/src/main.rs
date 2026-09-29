use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};
use std::{env, net::SocketAddr};

#[derive(Clone)]
struct AppState { id: String }

#[tokio::main]
async fn main() {
    let id = env::args().nth(1).unwrap_or_else(|| "backend".to_string());
    let port: u16 = env::args().nth(2).unwrap_or_else(|| "8001".to_string()).parse().unwrap();
    let state = AppState { id: id.clone() };
    let app = Router::new()
        .route("/health", get(health))
        .route("/api", get(api))
        .with_state(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("{} listening on http://{}", id, addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"status": "ok", "server": state.id}))
}

async fn api(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"message": "hello from MiniLB demo backend", "server": state.id}))
}
