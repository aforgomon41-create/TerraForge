//! TerraForge 服务端入口。

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let port: u16 = std::env::var("TF_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8790);

    tf_server::serve(port).await
}
