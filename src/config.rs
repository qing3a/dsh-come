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
    /// 启动后等待 HTTP 200 的最长秒数（就绪探测，启动阶段）。
    /// 首次安装/下载 dsh 依赖树可能较慢 → 默认 240s
    pub startup_timeout_secs: u64,
    /// 启动前自检模式（覆盖默认）：inspect(巡检)/treat(处置)/attend(主治)/emergency(急救)。
    /// 缺省 None → 首次启动用「处置」（只自愈零风险项，其余给推荐）；dsh 反复崩溃时监测线程会
    /// 无视此项、按崩溃次数逐级升级（处置→主治→急救）做兜底。
    #[serde(default)]
    pub doctor_mode: Option<String>,
    /// 轻量状态 HTTP 端点端口（0 = 关闭；默认 3081）。
    /// 浏览器访问 http://127.0.0.1:<port> 查看守护状态（实时 JSON，网页形态的状态管理）。
    #[serde(default = "default_status_port")]
    pub status_port: u16,
    /// 托盘「退出」时是否关闭 dsh 引擎（快捷菜单复选框「退出时关闭引擎」）。
    /// false = 退出时保留引擎运行（dsh 继续服务，下次启动自动认领）。
    #[serde(default = "default_true")]
    pub exit_close_engine: bool,
    /// 界面语言：zh / en（默认 zh；改动需重启生效，i18n.rs 首次调用时缓存）
    #[serde(default = "default_lang")]
    pub lang: String,
}

const DEFAULT_STATUS_PORT: u16 = 3081;

fn default_status_port() -> u16 {
    DEFAULT_STATUS_PORT
}

fn default_true() -> bool {
    true
}

fn default_lang() -> String {
    "zh".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 3080,
            host: "127.0.0.1".to_string(),
            max_restarts: 5,
            backoff_max_secs: 30,
            startup_timeout_secs: 240,
            doctor_mode: None,
            status_port: default_status_port(),
            exit_close_engine: true,
            lang: default_lang(),
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

/// 保存配置（写入失败留一行日志——配置是再生的默认值不阻塞主流程，但失败原因要可查）
pub fn save(cfg: &AppConfig) {
    let p = config_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(cfg) {
        if let Err(e) = std::fs::write(&p, s) {
            let _ = crate::supervisor::log(&format!("保存配置失败 {}: {e}", p.display()));
        }
    }
}
