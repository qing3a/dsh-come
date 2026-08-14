//! 启动器本地配置（config.json，缺省用默认值；改动需重启生效）。
//!
//! 与 dsh 的契约面只占 4 项（见 docs/cli-contract.md），这里只放启动器自己的行为参数，
//! 不存放任何 dsh 内部细节。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// dsh web 监听端口（与官方默认一致；启动器固定端口 → HTTP 健康探测）
    pub port: u16,
    /// 监听地址（默认仅本机回环，不对外暴露）
    pub host: String,
    /// 崩溃连续重启上限（防反复崩溃死循环；0 = 不自动重启）
    pub max_restarts: u32,
    /// 重启退避封顶（秒）：1,2,4,... 指数退避，封顶后不再增长
    pub backoff_max_secs: u64,
    /// 启动后等待 HTTP 200 的最长秒数（就绪探测）。
    /// 首次运行 npx 要完整下载 dsh 依赖树（含 koffi 等原生包），60s 偏紧 → 默认 240s
    pub startup_timeout_secs: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 3080,
            host: "127.0.0.1".to_string(),
            max_restarts: 5,
            backoff_max_secs: 30,
            startup_timeout_secs: 240,
        }
    }
}

pub fn config_path() -> std::path::PathBuf {
    crate::runtime::root_dir().join("config.json")
}

pub fn load() -> AppConfig {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

/// 保存配置（写入失败静默——配置都是可再生的默认值，不阻塞主流程）
pub fn save(cfg: &AppConfig) {
    let p = config_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&p, s);
    }
}
