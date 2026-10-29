use std::{net::IpAddr, path::PathBuf, time::Duration};
use tf_core::params::SimQuality;

#[derive(Clone)]
pub struct ServerConfig {
    pub bind: IpAddr,
    pub data_dir: PathBuf,
    pub static_dir: Option<PathBuf>,
    pub base_path: String,
    pub desktop: bool,
    pub quality: SimQuality,
    pub max_jobs: usize,
    pub max_queue: usize,
    pub session_ttl: Duration,
    pub admin_token: Option<String>,
}

impl ServerConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let desktop = std::env::var("TF_MODE").as_deref() == Ok("desktop");
        let quality = match std::env::var("TF_QUALITY")
            .unwrap_or_else(|_| "economy".into())
            .as_str()
        {
            "economy" | "low" => SimQuality::Economy,
            "standard" => SimQuality::Standard,
            "high" => SimQuality::High,
            other => anyhow::bail!("未知 TF_QUALITY: {other}"),
        };
        let quality = if desktop {
            quality
        } else {
            SimQuality::Economy
        };
        let base = std::env::var("TF_BASE_PATH").unwrap_or_else(|_| {
            if desktop {
                String::new()
            } else {
                "/map".into()
            }
        });
        let base_path = if base.trim_matches('/').is_empty() {
            String::new()
        } else {
            format!("/{}", base.trim_matches('/'))
        };
        anyhow::ensure!(
            !base_path.contains("..") && !base_path.contains('?'),
            "TF_BASE_PATH 无效"
        );
        Ok(Self {
            bind: std::env::var("TF_BIND")
                .unwrap_or_else(|_| "127.0.0.1".into())
                .parse()?,
            data_dir: std::env::var_os("TF_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| "./data".into()),
            static_dir: std::env::var_os("TF_STATIC_DIR").map(PathBuf::from),
            base_path,
            desktop,
            quality,
            max_jobs: env_usize("TF_MAX_CONCURRENT_JOBS", 1).clamp(1, 8),
            max_queue: env_usize("TF_MAX_QUEUE", 16).clamp(1, 128),
            session_ttl: Duration::from_secs(
                env_usize("TF_SESSION_TTL_SECS", 300).clamp(60, 86_400) as u64,
            ),
            admin_token: std::env::var("TF_ADMIN_TOKEN")
                .ok()
                .filter(|s| s.len() >= 32),
        })
    }

    pub fn max_patch_side(&self) -> u32 {
        match self.quality {
            SimQuality::Economy => 512,
            SimQuality::Standard => 1024,
            SimQuality::High => 2048,
        }
    }
}

fn env_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(fallback)
}
