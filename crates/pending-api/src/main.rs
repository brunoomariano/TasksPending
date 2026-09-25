use std::net::SocketAddr;

use anyhow::Context;
use axum::{Json, Router, routing::get};
use clap::Parser;
use pending_core::{DashboardSnapshot, sample_snapshot};
use serde::Serialize;
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "pending-api", about = "TasksPending HTTP API")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing();

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/snapshot", get(snapshot))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("binding {}", cli.listen))?;
    info!(addr = %cli.listen, "pending-api listening");

    axum::serve(listener, app).await.context("serving API")?;
    Ok(())
}

async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn snapshot() -> Json<DashboardSnapshot> {
    Json(sample_snapshot())
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "pending_api=info,tower_http=info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
