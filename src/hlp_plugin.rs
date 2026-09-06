//! HLP 插件（@hlp/dsh-light-cockpit）检测 / 安装 / 修复（v1.4.0 P0-2）。
//!
//! 部署位置 = DSH 共享层 `~/.dsh/profiles/node_modules/@hlp/dsh-light-cockpit`
//! （依赖链完整，见 runtime::hlp_plugin_dir 注释）。安装源 = exe 同目录的
//! `hlp-plugin\dsh-light-cockpit`（发行版布局）或 `hlp-plugin`（解压形态）。
//! 纪律：安装源缺失时报明确错误（不静默）；修复前备份 `.bak`，data/ 不在插件目录、不受影响。

use crate::runtime::{copy_dir_all, hlp_plugin_dir};
use std::path::PathBuf;

/// 已装版本号（读共享层 package.json 的 version）；未装/损坏 → None
pub fn detect_version() -> Option<String> {
    let manifest = hlp_plugin_dir().join("package.json");
    let text = std::fs::read_to_string(manifest).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("version")?.as_str().map(String::from)
}

/// 健康检查：manifest 可解析 + 入口/注册表/两个业务 Server 齐全
/// （business/*/node_modules 缺失时 MCP Server 起不来——healthy 必须覆盖到）
pub fn healthy() -> bool {
    let dir = hlp_plugin_dir();
    dir.join("package.json").is_file()
        && dir.join("index.js").is_file()
        && dir.join("lib").is_dir()
        && dir
            .join("business")
            .join("biz-cockpit-mcp")
            .join("package.json")
            .is_file()
        && dir
            .join("business")
            .join("mail-collab-server")
            .join("package.json")
            .is_file()
}

/// 安装源候选（按序）：exe 同目录 hlp-plugin\dsh-light-cockpit / hlp-plugin
fn source_candidates() -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    match exe_dir {
        Some(d) => vec![
            d.join("hlp-plugin").join("dsh-light-cockpit"),
            d.join("hlp-plugin"),
        ],
        None => vec![],
    }
}

/// 从安装源复制插件到共享层。返回复制的文件数。
fn deploy_from_source() -> Result<u64, String> {
    for src in source_candidates() {
        if !src.join("package.json").is_file() {
            continue;
        }
        let dst = hlp_plugin_dir();
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建共享层目录失败：{e}"))?;
        }
        if dst.exists() {
            std::fs::remove_dir_all(&dst).map_err(|e| format!("清理旧插件目录失败：{e}"))?;
        }
        let n = copy_dir_all(&src, &dst).map_err(|e| format!("复制插件失败：{e}"))?;
        if !healthy() {
            return Err("复制完成但健康检查未通过（源不完整？）".to_string());
        }
        return Ok(n);
    }
    Err(format!(
        "未找到安装源：exe 同目录应有 hlp-plugin\\dsh-light-cockpit（发行版应附带；当前 exe 目录：{}）",
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.display().to_string()))
            .unwrap_or_default()
    ))
}

/// 安装（已装时报错提示用修复）
pub fn install() -> Result<String, String> {
    if detect_version().is_some() {
        return Err(format!(
            "HLP 插件已安装（v{}）；如需重装请用修复",
            detect_version().unwrap_or_default()
        ));
    }
    let n = deploy_from_source()?;
    Ok(format!(
        "安装完成（复制 {n} 个文件到 {}，版本 {}）",
        hlp_plugin_dir().display(),
        detect_version().unwrap_or_else(|| "?".into())
    ))
}

/// 修复：备份旧目录（.bak，先清上一次备份）→ 删除 → 重新部署。
/// 注意：dsh 引擎运行中会锁住插件文件（Windows 目录 rename 失败）——
/// 备份失败时错误信息明确指引先停引擎（管理页 /api/stop）。
pub fn repair() -> Result<String, String> {
    let dir = hlp_plugin_dir();
    if dir.exists() {
        let bak = dir.with_extension("bak");
        if bak.exists() {
            std::fs::remove_dir_all(&bak).map_err(|e| format!("清理旧备份失败：{e}"))?;
        }
        std::fs::rename(&dir, &bak).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "备份失败：插件目录被占用（dsh 引擎运行中）——请先在管理页停止 dsh 引擎，再点修复"
                    .to_string()
            } else {
                format!("备份失败：{e}")
            }
        })?;
        match deploy_from_source() {
            Ok(n) => Ok(format!(
                "修复完成（旧目录备份为 {}，复制 {n} 个文件）",
                bak.display()
            )),
            Err(e) => {
                // 回滚：新装失败则还原备份，绝不留半残状态
                if !dir.exists() && bak.exists() {
                    let _ = std::fs::rename(&bak, &dir);
                }
                Err(format!("修复失败（已回滚备份）：{e}"))
            }
        }
    } else {
        let n = deploy_from_source()?;
        Ok(format!("原目录不存在，直接安装完成（{n} 个文件）"))
    }
}

/// 综合状态（管理页 /api/hlp/status）
pub fn status_json() -> serde_json::Value {
    let dir = hlp_plugin_dir();
    serde_json::json!({
        "installed": detect_version().is_some(),
        "version": detect_version(),
        "path": dir.display().to_string(),
        "healthy": healthy(),
        "bundled": source_candidates().iter().any(|p| p.join("package.json").is_file()),
    })
}
