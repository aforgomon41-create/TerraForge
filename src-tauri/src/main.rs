//! The packaged UI and API are served together, on one reserved loopback socket.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, TcpListener, TcpStream},
    time::{Duration, Instant},
};
use tauri::Manager;

fn wait_until_ready(address: std::net::SocketAddr) -> bool {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = write!(
                stream,
                "GET /api/v1/health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
            );
            let mut buffer = [0; 64];
            if let Ok(n) = stream.read(&mut buffer) {
                if String::from_utf8_lossy(&buffer[..n]).starts_with("HTTP/1.1 200") {
                    return true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let data = std::env::var_os("TF_DESKTOP_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&data)?;
            // Bind port 0 once, then transfer this very socket to axum. No scan/rebind race.
            let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            socket.set_nonblocking(true)?;
            let address = socket.local_addr()?;
            let mut config = tf_server::config::ServerConfig::from_env()?;
            config.bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
            config.desktop = true;
            config.data_dir = data.clone();
            config.base_path.clear();
            config.static_dir = None;
            config.quality = match std::env::var("TF_DESKTOP_QUALITY").as_deref() {
                Ok("high") => tf_core::params::SimQuality::High,
                Ok("economy") => tf_core::params::SimQuality::Economy,
                _ => tf_core::params::SimQuality::Standard,
            };
            config.max_jobs = 1;
            let resolver = std::sync::Arc::new(app.asset_resolver());
            let web =
                axum::Router::new().fallback(axum::routing::get(move |uri: axum::http::Uri| {
                    let resolver = resolver.clone();
                    async move {
                        let path = uri.path().trim_start_matches('/');
                        let path = if path.is_empty() { "index.html" } else { path };
                        if path.split('/').any(|part| part == "..") {
                            return axum::http::Response::builder()
                                .status(400)
                                .body(axum::body::Body::empty())
                                .unwrap();
                        }
                        match resolver.get(path.to_string()) {
                            Some(asset) => axum::http::Response::builder()
                                .header("content-type", asset.mime_type)
                                .header("cache-control", "no-cache")
                                .body(axum::body::Body::from(asset.bytes))
                                .unwrap(),
                            None => axum::http::Response::builder()
                                .status(404)
                                .body(axum::body::Body::from("资源不存在"))
                                .unwrap(),
                        }
                    }
                }));
            let error_log = data.join("startup-error.log");
            std::thread::spawn(move || {
                let result = (|| -> anyhow::Result<()> {
                    let rt = tokio::runtime::Runtime::new()?;
                    rt.block_on(async move {
                        let listener = tokio::net::TcpListener::from_std(socket)?;
                        tf_server::serve_listener(listener, config, Some(web)).await
                    })
                })();
                if let Err(error) = result {
                    let _ = std::fs::write(error_log, format!("{error:#}"));
                }
            });
            if !wait_until_ready(address) {
                return Err(
                    "本地地图服务未能启动，请查看应用数据目录中的 startup-error.log".into(),
                );
            }
            let window = app.get_webview_window("main").ok_or("主窗口不存在")?;
            window.navigate(format!("http://{address}/").parse()?)?;
            window.show()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("TerraForge 启动失败");
}
