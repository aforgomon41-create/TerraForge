//! TerraForge 桌面版入口。
//!
//! 启动时同时运行前端窗口和后端服务：
//! - 前端是打包进二进制的 Web 资源
//! - 后端是同一个 Rust 进程里的 axum 服务
//!
//! 这样桌面版是真正的单二进制，用户不需要装任何东西。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::PathBuf;

fn main() {
    // 桌面版的数据目录用系统标准位置
    let data_dir = dirs_data_dir();
    std::fs::create_dir_all(&data_dir).ok();

    // 找一个空闲端口
    let port = find_free_port(8790);

    std::env::set_var("TF_DATA_DIR", data_dir.to_string_lossy().to_string());
    std::env::set_var("TF_PORT", port.to_string());
    std::env::set_var("TF_QUALITY", "high");
    std::env::set_var("TF_MAX_CONCURRENT_JOBS", "2");

    // 在后台线程启动后端
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("无法创建 tokio 运行时");
        rt.block_on(async move {
            if let Err(e) = tf_server::serve(port).await {
                eprintln!("后端启动失败: {}", e);
            }
        });
    });

    // 等后端就绪
    std::thread::sleep(std::time::Duration::from_millis(800));

    // 启动前端窗口，指向本地后端
    let url = format!("http://127.0.0.1:{}/", port);

    tauri::Builder::default()
        .setup(move |app| {
            use tauri::Manager;
            let window = app.get_webview_window("main").expect("找不到主窗口");
            window.navigate(url.parse().expect("URL 无效"));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 启动失败");
}

/// 桌面版数据目录：Windows 用 %APPDATA%，其他用 ~/.local/share
fn dirs_data_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata).join("TerraForge")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/terraforge")
    } else {
        PathBuf::from("./terraforge-data")
    }
}

/// 从给定端口开始找空闲的。
fn find_free_port(start: u16) -> u16 {
    for p in start..start + 20 {
        if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return p;
        }
    }
    start
}
