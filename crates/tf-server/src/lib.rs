//! Shared HTTP runtime for web and desktop deployments.
pub mod api;
pub mod auth;
pub mod child_gen;
pub mod circulation;
pub mod climatology;
pub mod config;
pub mod cryosphere;
pub mod ecosystems;
pub mod generate;
pub mod geography;
pub mod hydrology;
pub mod jobs;
mod project;
pub mod render;
pub mod store;
mod surface;
pub mod tectonics;

use api::AppState;
use axum::Router;
use config::ServerConfig;
use jobs::JobManager;
use std::sync::Arc;
use store::Store;

pub async fn serve(port: u16) -> anyhow::Result<()> {
    let config = ServerConfig::from_env()?;
    let listener = tokio::net::TcpListener::bind((config.bind, port)).await?;
    serve_listener(listener, config, None).await
}

pub async fn serve_listener(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
    web: Option<Router>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data_dir)?;
    let store = Store::open(config.data_dir.join("terraforge.db"))?;
    let jobs = JobManager::with_limits(store.clone(), config.max_jobs, config.max_queue);
    jobs.recover()?;
    let state = AppState {
        store,
        jobs,
        config: Arc::new(config),
        exports: Arc::new(tokio::sync::Semaphore::new(1)),
        reads: Arc::new(tokio::sync::Semaphore::new(8)),
        streams: Arc::new(tokio::sync::Semaphore::new(32)),
        admission: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let app = application(&state, web)?;
    let maintenance = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if state.config.desktop {
                continue;
            }
            let sessions = match state.store.sessions() {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("读取会话失败: {e}");
                    continue;
                }
            };
            for session in sessions {
                if jobs::now_millis().saturating_sub(session.last_seen)
                    >= state.config.session_ttl.as_millis() as u64
                {
                    if let Err(e) = api::expire_if_idle(&state, &session.id).await {
                        tracing::error!("清理会话失败: {e}");
                    }
                }
            }
        }
    });
    tracing::info!("TerraForge listening on {}", listener.local_addr()?);
    let result = axum::serve(listener, app).await;
    maintenance.abort();
    result?;
    Ok(())
}

/// Public application assembly is also used by deployment-path regression tests.
pub fn application(state: &AppState, web: Option<Router>) -> anyhow::Result<Router> {
    let mut app = api::build_router(state.clone());
    if let Some(web) = web {
        app = app.fallback_service(web)
    } else if let Some(dir) = &state.config.static_dir {
        anyhow::ensure!(dir.join("index.html").exists(), "前端目录没有 index.html");
        app = app.fallback_service(tower_http::services::ServeDir::new(dir));
    }
    let base = &state.config.base_path;
    if !base.is_empty() {
        app = app.clone().nest(base, app);
        if let Some(dir) = &state.config.static_dir {
            let target = format!("{base}/");
            app = app
                .route(
                    base,
                    axum::routing::get(move || async move {
                        axum::response::Redirect::temporary(&target)
                    }),
                )
                .route_service(
                    &format!("{base}/"),
                    tower_http::services::ServeFile::new(dir.join("index.html")),
                );
        }
    }
    Ok(app.layer(tower_http::compression::CompressionLayer::new()))
}

#[cfg(test)]
mod tests;
