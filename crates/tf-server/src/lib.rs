//! TerraForge 服务端库。
//!
//! 提供两种用法：
//! 1. 独立二进制（`tf-server`，见 main.rs）
//! 2. 被桌面版内嵌（Tauri，调用 `serve`）

pub mod api;
pub mod child_gen;
pub mod generate;
pub mod jobs;
pub mod render;
pub mod store;

use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::info;

use api::AppState;
use jobs::JobManager;
use store::Store;

/// 启动 HTTP 服务。
///
/// 配置全部从环境变量读取，和独立二进制保持一致。
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let data_dir = std::env::var("TF_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"));
    std::fs::create_dir_all(&data_dir)?;

    let max_jobs: usize = std::env::var("TF_MAX_CONCURRENT_JOBS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let static_dir = std::env::var("TF_STATIC_DIR").map(PathBuf::from).ok();

    info!("数据目录: {}", data_dir.display());
    info!("并发上限: {}", max_jobs);

    let db_path = data_dir.join("terraforge.db");
    let store = Store::open(&db_path)?;
    let jobs = JobManager::new(store.clone(), max_jobs);

    let state = AppState { store, jobs };
    let mut router = api::build_router(state);

    if let Some(dir) = static_dir {
        if dir.exists() {
            info!("静态资源目录: {}", dir.display());
            router = router.fallback_service(
                tower_http::services::ServeDir::new(&dir).append_index_html_on_directories(true),
            );
        }
    }

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("监听 http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
