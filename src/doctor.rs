//! 证据驱动的自愈诊疗（doctor）：扫描取证 → 推理分级 → 按模式授权处置 → 兜底升级。
//!
//! 设计要点（用户诉求）：
//! - **不写死检查**：所有「发现」都来自对环境的实际扫描与推理，而不是硬编码「md-agent 导致」。
//!   即便将来是别的插件 / 别的原因把 dsh 拖垮，诊疗也能靠「孤儿 file:// 入口 / 损坏配置 /
//!   残缺下载 / 端口被占 / 孤儿进程」这些**证据**识别出来。
//! - **先检测 → 再推荐方案 → 最后兜底**：默认只做零风险自愈（🟢绿），其余给出可执行的推荐；
//!   当 dsh 反复崩溃、常规手段都救不活时，逐级升级到「急救」（替代「严苛」字眼）做最后兜底。
//! - **可清理**：残缺下载（.tmp/.partial/.crdownload/_downloads）、孤儿 dsh/node 进程、
//!   损坏或被孤立引用的配置（cordis.patch.yml 里的死 file:// 入口）。
//! - **影响半径分级**：
//!   🟢 绿 = 可逆、零风险、可自动（如重建壳自有 come.patch.yml）；
//!   🟡 黄 = 需确认 / 仅「主治」及以上自动（如结束占用端口的进程、删孤儿配置条目）；
//!   🔴 红 = 必须先备份再动 / 仅「急救」自动（如重置损坏的 profile patch）。
//! - **模式阶梯**（避免「严苛」字眼，语义更温和也更准确）：
//!   巡检 Inspect（只报不改）/ 处置 Treat（自动绿，黄红只推荐）/ 主治 Attend（自动绿+黄，红只推荐）/
//!   急救 Emergency（全量，红色必先备份）—— 即「兜底方案」。

use crate::config::AppConfig;
use crate::patchyml::{file_uri_path, looks_like_patch, parse_entries, remove_entry};
use crate::runtime;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;

// ===================== 模式 =====================

/// 诊疗模式（分级阶梯；数值序即强度序，便于 `>=` 比较授权）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    /// 巡检：只扫描上报，任何东西都不改
    Inspect,
    /// 处置：自动修 🟢绿，🟡黄 / 🔴红 只推荐
    Treat,
    /// 主治：自动修 🟢绿 + 🟡黄（配置改动前先备份），🔴红 只推荐
    Attend,
    /// 急救：全量处置，🔴红 会先备份再动——最后兜底
    Emergency,
}

impl Mode {
    pub fn from_str(s: &str) -> Option<Mode> {
        match s.to_ascii_lowercase().as_str() {
            "inspect" | "巡检" => Some(Mode::Inspect),
            "treat" | "处置" => Some(Mode::Treat),
            "attend" | "主治" => Some(Mode::Attend),
            "emergency" | "急救" => Some(Mode::Emergency),
            _ => None,
        }
    }

    /// 失败时逐级升级（兜底）：巡检→处置→主治→急救→急救
    pub fn escalate(self) -> Mode {
        match self {
            Mode::Inspect => Mode::Treat,
            Mode::Treat => Mode::Attend,
            Mode::Attend => Mode::Emergency,
            Mode::Emergency => Mode::Emergency,
        }
    }

    /// 第 n 次崩溃（1-based）对应的升级模式：1→处置，2→主治，≥3→急救
    pub fn for_restart(n: u32) -> Mode {
        match n {
            1 => Mode::Treat,
            2 => Mode::Attend,
            _ => Mode::Emergency,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Inspect => "巡检",
            Mode::Treat => "处置",
            Mode::Attend => "主治",
            Mode::Emergency => "急救",
        }
    }
}

// ===================== 影响半径 =====================

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Blast {
    Green,
    Yellow,
    Red,
}

impl Blast {
    pub fn mark(self) -> &'static str {
        match self {
            Blast::Green => "🟢",
            Blast::Yellow => "🟡",
            Blast::Red => "🔴",
        }
    }
}

/// 该半径在当前模式下是否「自动执行」
fn should_auto(blast: Blast, mode: Mode) -> bool {
    match blast {
        Blast::Green => mode != Mode::Inspect,
        Blast::Yellow => mode >= Mode::Attend,
        Blast::Red => mode == Mode::Emergency,
    }
}

// ===================== 修复动作（惰性描述；apply 时才落地） =====================

pub enum Remedy {
    /// 重建壳自有 come.patch.yml（可完整重建，零风险）
    EnsureComePatch,
    /// 结束占用端口的进程（Yellow；attributable=true 时主治即可自动，否则仅急救）
    RemovePortHolder { pid: u32, attributable: bool },
    /// 从 profile 的 cordis.patch.yml 删除某个孤儿条目（按 id）
    EditProfilePatchRemoveEntry { entry_id: String },
    /// 备份并重置损坏的 cordis.patch.yml 为最小可用（Red）
    BackupAndResetProfilePatch,
    /// 清理 .dsh 下的残缺下载临时文件/目录（路径在构造时收集好）
    CleanPartialDownloads { paths: Vec<PathBuf> },
    /// 结束孤儿 dsh/node 进程（pid 列表）
    KillOrphan { pids: Vec<u32> },
}

fn remedy_desc(r: &Remedy) -> &'static str {
    match r {
        Remedy::EnsureComePatch => "重建壳自有 come.patch.yml（可完整重建，零风险）",
        Remedy::RemovePortHolder { .. } => "结束占用端口的进程",
        Remedy::EditProfilePatchRemoveEntry { .. } => "从 cordis.patch.yml 移除孤儿条目",
        Remedy::BackupAndResetProfilePatch => "备份并重置 cordis.patch.yml 为最小可用",
        Remedy::CleanPartialDownloads { .. } => "清理 .dsh 下的残缺下载临时文件",
        Remedy::KillOrphan { .. } => "结束孤儿 dsh/node 进程",
    }
}

// ===================== 一条发现（证据驱动） =====================

pub struct Finding {
    pub id: &'static str,
    pub title: String,
    pub evidence: String,
    pub blast: Blast,
    pub remedy: Option<Remedy>,
}

// ===================== 入口：heal =====================

/// 诊疗总入口：扫描 → （非巡检）按模式授权处置 → 写报告到 engine.log。
/// 由 `run_first_boot`（首次启动，默认处置）与 `supervisor` 监测线程（按崩溃次数升级）调用。
pub fn heal(cfg: &AppConfig, mode: Mode) {
    let findings = scan_all(cfg);
    let report = build_report(cfg, mode, &findings);
    crate::supervisor::log(&report);

    if mode == Mode::Inspect {
        return; // 只报不改
    }

    let (applied, recs) = apply_all(&findings, mode);
    if !applied.is_empty() {
        crate::supervisor::log(&summarize(&applied));
    }
    for r in &recs {
        crate::supervisor::log(r);
    }
}

/// 命令行 `dsh-come doctor [--mode X]`：打印报告；非巡检模式实际落地。
pub fn run_cli(cfg: &AppConfig, mode: Mode) {
    let findings = scan_all(cfg);
    let report = build_report(cfg, mode, &findings);
    println!("{report}");
    crate::supervisor::log(&report);

    if mode == Mode::Inspect {
        println!("\n（巡检模式：未做任何改动。加 --mode attend/emergency 可执行修复）");
        return;
    }
    let (applied, recs) = apply_all(&findings, mode);
    for (t, res) in &applied {
        match res {
            Ok(m) => println!("✅ {t}：{m}"),
            Err(e) => println!("❌ {t}：{e}"),
        }
    }
    for r in &recs {
        println!("➡️  {r}");
    }
}

// ===================== 扫描（全部读-only，不改动任何状态） =====================

fn scan_all(cfg: &AppConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    probe_runner(&mut out);
    probe_port(cfg, &mut out);
    probe_come_patch(&mut out);
    probe_profile_patch(&mut out);
    probe_partial_downloads(&mut out);
    probe_orphan_processes(cfg, &mut out);
    out
}

/// 1) 运行器缺失：找不到系统 dsh → 无法启动（不可自愈，仅上报，提示安装）
fn probe_runner(out: &mut Vec<Finding>) {
    if runtime::dsh_runner().is_none() {
        out.push(Finding {
            id: "no-runner",
            title: "未找到系统 dsh".to_string(),
            evidence: "PATH 中无 dsh 命令。无法 spawn dsh 引擎，请先安装（管理页/向导会自动安装）。".to_string(),
            blast: Blast::Red,
            remedy: None, // 需要用户安装 Node.js / dsh，诊疗无法代装
        });
    }
}

/// 2) 端口冲突：本机端口已被别的进程监听 → dsh web 起不来。
/// 区分两种情况（与 supervisor 认领机制协调）：
/// - 占用者**健康**（HTTP 200）→ 壳启动时会认领它而非重复启动，**不杀**，只提示；
/// - 占用者**不健康**（僵尸 node 占着端口）→ 按分级处置（🟡黄/🔴红）杀掉腾出端口。
fn probe_port(cfg: &AppConfig, out: &mut Vec<Finding>) {
    if let Some(pid) = listening_pid_on_port(cfg.port) {
        let attributable = is_our_process(pid, cfg);
        if crate::supervisor::http_ok(cfg.port, 1000) {
            out.push(Finding {
                id: "port-healthy-claimed",
                title: format!("端口 {} 已有健康 dsh 运行（pid={}），将接管而非重复启动", cfg.port, pid),
                evidence: format!("HTTP 200 且端口探测显示 127.0.0.1:{} 被 PID {pid} 监听", cfg.port),
                blast: Blast::Green,
                remedy: None, // start() 的认领逻辑处理，无需处置
            });
        } else {
            out.push(Finding {
                id: "port-conflict",
                title: format!("端口 {} 被不健康进程占用，dsh web 无法绑定", cfg.port),
                evidence: format!("端口探测显示 127.0.0.1:{} 已被 PID {} 监听（LISTENING），但 HTTP 不响应——可能是僵尸 node 残留", cfg.port, pid),
                blast: Blast::Yellow,
                remedy: Some(Remedy::RemovePortHolder { pid, attributable }),
            });
        }
    }
}

/// 3) 壳自有 come.patch.yml：缺失或内容异常 → 可重建（🟢绿）
fn probe_come_patch(out: &mut Vec<Finding>) {
    let p = runtime::come_patch_path();
    if !p.is_file() {
        out.push(Finding {
            id: "come-patch-missing",
            title: "壳 patch 文件 come.patch.yml 缺失".to_string(),
            evidence: format!("{} 不存在；dsh 启动将不带壳 overlay", p.display()),
            blast: Blast::Green,
            remedy: Some(Remedy::EnsureComePatch),
        });
        return;
    }
    if let Ok(text) = std::fs::read_to_string(&p) {
        if !looks_like_patch(&text) {
            out.push(Finding {
                id: "come-patch-corrupt",
                title: "壳 patch 文件 come.patch.yml 内容异常".to_string(),
                evidence: format!("{} 存在但结构不像合法 patch 列表，可能被截断/损坏", p.display()),
                blast: Blast::Green,
                remedy: Some(Remedy::EnsureComePatch),
            });
        }
    }
}

/// 4) dsh 的 profile patch（cordis.patch.yml）：孤儿 file:// 入口 / 损坏 → 这是「联动拖垮 dsh」的主因
fn probe_profile_patch(out: &mut Vec<Finding>) {
    let p = profile_patch_path();
    if !p.exists() {
        return; // 没这份 patch → dsh 用默认，无需处理
    }
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) => {
            out.push(Finding {
                id: "profile-patch-unreadable",
                title: "cordis.patch.yml 无法读取".to_string(),
                evidence: format!("读取 {} 失败：{e}", p.display()),
                blast: Blast::Red,
                remedy: Some(Remedy::BackupAndResetProfilePatch),
            });
            return;
        }
    };

    if !looks_like_patch(&text) {
        out.push(Finding {
            id: "profile-patch-corrupt",
            title: "cordis.patch.yml 损坏，无法解析为合法 patch 列表".to_string(),
            evidence: format!("{} 结构异常（顶层出现非列表项），dsh 加载时会整树失败", p.display()),
            blast: Blast::Red,
            remedy: Some(Remedy::BackupAndResetProfilePatch),
        });
        return;
    }

    // 逐条检查：file:// 指向的路径是否还存在（不存在 = 孤儿入口，加载即拖垮 dsh）
    for (entry_id, src) in orphan_file_entries(&text) {
        out.push(Finding {
            id: "orphan-file-plugin",
            title: format!("插件入口 `{entry_id}` 指向不存在的路径（孤儿配置）"),
            evidence: format!("cordis.patch.yml 中 `{entry_id}` 的源 `{src}` 在磁盘上不存在；cordis 严格模式下该条加载失败会拖垮整个 dsh 启动"),
            blast: Blast::Yellow,
            remedy: Some(Remedy::EditProfilePatchRemoveEntry { entry_id }),
        });
    }
}

/// 5) 残缺下载：扫描 dsh 数据根 + 启动器根两处（不写死单一目录）
fn probe_partial_downloads(out: &mut Vec<Finding>) {
    // 两处根都扫：dsh 数据根（.dsh）与启动器根（dsh-come），去重合并成一条发现
    let roots = [runtime::system_home_dir(), runtime::root_dir()];
    let mut seen = std::collections::HashSet::new();
    let mut junk: Vec<PathBuf> = Vec::new();
    let mut sample: Vec<String> = Vec::new();
    for r in &roots {
        for p in collect_junk(r, 150) {
            if seen.insert(p.clone()) {
                if sample.len() < 3 {
                    sample.push(
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                    );
                }
                junk.push(p);
            }
        }
    }
    if !junk.is_empty() {
        out.push(Finding {
            id: "partial-downloads",
            title: format!("发现 {} 个残缺下载/临时文件", junk.len()),
            evidence: format!("分布于 dsh 数据根与启动器根下，例如：{}", sample.join("、")),
            blast: Blast::Yellow,
            remedy: Some(Remedy::CleanPartialDownloads { paths: junk }),
        });
    }
}

/// 6) 孤儿进程：命令行列含 dsh / 本端口 / .dsh 的 node/dsh 残留进程。
/// 分级（防误杀）：
/// - **活引擎树**（supervisor 当前管理的进程树）一律排除，不碰；
/// - 命令行含**本端口**（极可能是上次崩溃残留、占着端口的引擎）→ 🟡黄，主治及以上自动；
/// - 仅名字/命令行像 dsh、无端口证据（可能是用户另开的 dsh 项目）→ 🔴红，仅急救自动。
fn probe_orphan_processes(cfg: &AppConfig, out: &mut Vec<Finding>) {
    let self_pid = std::process::id();
    let engine_tree = engine_tree_pids();
    let mut port_related: Vec<u32> = Vec::new();
    let mut suspect: Vec<u32> = Vec::new();
    for (pid, _, name, cl) in ps_table() {
        if pid == self_pid || engine_tree.contains(&pid) {
            continue; // 自己 / 活引擎树，不碰
        }
        if !name_matches_dsh(&name) {
            continue; // 只关心 node/dsh 进程
        }
        if !(cl.contains("dsh") || cl.contains(".dsh")) {
            continue; // 命令行与 dsh 无关（node.exe 很多，别误伤）
        }
        if cl.contains(&cfg.port.to_string()) {
            port_related.push(pid);
        } else {
            suspect.push(pid);
        }
    }
    if !port_related.is_empty() {
        out.push(Finding {
            id: "orphan-processes",
            title: format!("发现 {} 个孤儿 dsh/node 进程（占着本端口）", port_related.len()),
            evidence: format!("PID {:?} 命令行含端口 {}，可能是上次崩溃残留，会阻止 dsh 绑定端口", port_related, cfg.port),
            blast: Blast::Yellow,
            remedy: Some(Remedy::KillOrphan { pids: port_related }),
        });
    }
    if !suspect.is_empty() {
        out.push(Finding {
            id: "orphan-processes-suspect",
            title: format!("发现 {} 个疑似 dsh 进程（无端口证据，可能是其他 dsh 实例）", suspect.len()),
            evidence: format!("PID {:?} 命令行含 dsh 但不含本端口，结束前请确认不是正在使用的实例", suspect),
            blast: Blast::Red,
            remedy: Some(Remedy::KillOrphan { pids: suspect }),
        });
    }
}

// ===================== 处置落地 =====================

/// 按模式逐条处置；返回（已执行结果, 推荐项）
fn apply_all(
    findings: &[Finding],
    mode: Mode,
) -> (Vec<(String, Result<String, String>)>, Vec<String>) {
    let mut applied = Vec::new();
    let mut recs = Vec::new();
    for f in findings {
        if let Some(rem) = &f.remedy {
            // 端口占用且进程不可归属（非我们的 dsh）→ 仅急救才自动结束，避免误杀他进程
            let auto = match rem {
                Remedy::RemovePortHolder { attributable, .. } if !*attributable => {
                    mode == Mode::Emergency
                }
                _ => should_auto(f.blast, mode),
            };
            if auto {
                let res = apply_remedy(rem);
                applied.push((f.title.clone(), res));
            } else if mode != Mode::Inspect {
                recs.push(format!(
                    "{} {}：建议执行——{}",
                    f.blast.mark(),
                    f.title,
                    remedy_desc(rem)
                ));
            }
        }
    }
    (applied, recs)
}

fn apply_remedy(r: &Remedy) -> Result<String, String> {
    match r {
        Remedy::EnsureComePatch => {
            runtime::ensure_come_patch().map_err(|e| e.to_string())?;
            Ok("已确保/重建 come.patch.yml（壳自有，可重建）".to_string())
        }
        Remedy::RemovePortHolder { pid, .. } => {
            kill_pid(*pid)?;
            Ok(format!("已结束占用端口的进程 PID {pid}"))
        }
        Remedy::EditProfilePatchRemoveEntry { entry_id } => {
            let p = profile_patch_path();
            let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
            let new_text = remove_entry(&text, entry_id)
                .ok_or_else(|| format!("在 {} 中未找到条目 {entry_id}", p.display()))?;
            backup(&p)?;
            std::fs::write(&p, new_text).map_err(|e| e.to_string())?;
            Ok(format!("已从 cordis.patch.yml 移除孤儿条目 `{entry_id}`（原文件已备份为 .bak）"))
        }
        Remedy::BackupAndResetProfilePatch => {
            let p = profile_patch_path();
            if p.exists() {
                backup(&p)?;
            }
            std::fs::write(
                &p,
                "# 由 dsh-come 急救重置（原文件已备份为 .bak）\n[]\n",
            )
            .map_err(|e| e.to_string())?;
            Ok("已备份并重置 cordis.patch.yml 为最小可用（空列表）".to_string())
        }
        Remedy::CleanPartialDownloads { paths } => {
            let mut n = 0;
            for p in paths {
                let ok = if p.is_dir() {
                    std::fs::remove_dir_all(p).is_ok()
                } else {
                    std::fs::remove_file(p).is_ok()
                };
                if ok {
                    n += 1;
                }
            }
            Ok(format!("已清理 {n} 个残缺下载文件/目录"))
        }
        Remedy::KillOrphan { pids } => {
            let mut n = 0;
            for pid in pids {
                if kill_pid(*pid).is_ok() {
                    n += 1;
                }
            }
            Ok(format!("已结束 {n} 个孤儿进程"))
        }
    }
}

// ===================== 报告文本 =====================

fn build_report(cfg: &AppConfig, mode: Mode, findings: &[Finding]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "【DSH 伴侣·诊疗报告】模式={}（{}）",
        mode.label(),
        match mode {
            Mode::Inspect => "只检测不上手",
            Mode::Treat => "自愈零风险项，其余给推荐",
            Mode::Attend => "自愈绿+黄，红给推荐",
            Mode::Emergency => "全量兜底（红先备份）",
        }
    ));
    if findings.is_empty() {
        s.push_str("\n✅ 未发现需处置的异常：运行器/端口/配置/下载/进程 均正常");
        return s;
    }
    for f in findings {
        s.push_str(&format!("\n{} [{}] {}", f.blast.mark(), f.id, f.title));
        s.push_str(&format!("\n   证据：{}", f.evidence));
        if let Some(r) = &f.remedy {
            let how = if should_auto(f.blast, mode) {
                "将自动执行"
            } else {
                "仅推荐（当前模式不自动）"
            };
            s.push_str(&format!("\n   处置：{}（{how}）", remedy_desc(r)));
        } else {
            s.push_str("\n   处置：需用户手动解决（诊疗无法代劳）");
        }
    }
    let _ = cfg;
    s
}

fn summarize(results: &[(String, Result<String, String>)]) -> String {
    let mut s = String::from("【诊疗执行结果】");
    for (t, res) in results {
        match res {
            Ok(m) => s.push_str(&format!("\n  ✅ {t}：{m}")),
            Err(e) => s.push_str(&format!("\n  ❌ {t}：{e}")),
        }
    }
    s
}

// ===================== 系统交互辅助 =====================

/// 定位 dsh 的 profile patch（cordis.patch.yml）。
/// 不写死绝对路径——从环境变量解析 dsh 根目录后，在根内「扫描」实际位置：
/// 1) 约定路径 `<home>/.dsh/profiles/web/cordis.patch.yml`（最常见）
/// 2) 否则在 dsh 根目录内递归扫描同名文件（有界深度，防大目录卡死）
/// 3) 同样扫描启动器根目录（部分布局 patch 可能在 dsh-come 下）
/// 4) 都没命中则回退约定路径——上层据此检测为「无 patch」而跳过
fn profile_patch_path() -> PathBuf {
    let home = runtime::system_home_dir();
    let conventional = home.join("profiles").join("web").join("cordis.patch.yml");
    if conventional.is_file() {
        return conventional;
    }
    if let Some(found) = scan_named(&home, "cordis.patch.yml", 6) {
        return found;
    }
    let desktop = runtime::root_dir();
    if desktop != home {
        if let Some(found) = scan_named(&desktop, "cordis.patch.yml", 6) {
            return found;
        }
    }
    conventional
}

/// 在 dir 内递归查找名为 `name` 的文件（有界深度，命中首个即返回）。
fn scan_named(dir: &Path, name: &str, max_depth: usize) -> Option<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: usize, out: &mut Option<PathBuf>) {
        if depth == 0 || out.is_some() {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name() == name && p.is_file() {
                *out = Some(p);
                return;
            }
            if p.is_dir() {
                walk(&p, name, depth - 1, out);
                if out.is_some() {
                    return;
                }
            }
        }
    }
    let mut found = None;
    walk(dir, name, max_depth, &mut found);
    found
}

/// 备份文件为 `<name>.bak`（覆盖式；只是兜底备份，非版本历史）
fn backup(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Ok(());
    }
    let bak = p.with_extension("bak");
    std::fs::copy(p, &bak).map(|_| ()).map_err(|e| format!("备份 {} 失败：{e}", p.display()))
}

fn kill_pid(pid: u32) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()]);
        crate::supervisor::hide_window(&mut cmd);
        let status = cmd.status().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("taskkill PID {pid} 返回非零（可能已退出或无权限）"))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // SAFETY: kill 信号调用；pid>0 单进程（孤儿清理，非进程组——组杀在 supervisor::kill_tree）
        let r = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if r == 0 {
            // 等 2s 优雅退出，未果 SIGKILL
            std::thread::sleep(std::time::Duration::from_secs(2));
            // SAFETY: 0 号信号探测存活；进程不存在返回 -1
            let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
            if alive {
                // SAFETY: 同上
                let k = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                if k != 0 {
                    return Err(format!("kill -9 PID {pid} 失败（可能已退出或无权限）"));
                }
            }
            Ok(())
        } else {
            Err(format!("kill PID {pid} 失败（可能已退出或无权限）"))
        }
    }
}

/// 捕获命令 stdout（Windows 下经 cmd /C；隐藏窗口）
fn capture(mut cmd: Command) -> Option<String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    crate::supervisor::hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 查端口被谁监听（平台化），返回占用 PID。
/// - Windows：`cmd /C netstat -ano -p tcp`。
/// - Linux：直读 `/proc/net/tcp`(+tcp6) + `/proc/<pid>/fd`（见 `listening_pid_linux`），
///   不依赖 ss——容器与精简系统常缺 ss，且 /proc 直读天然免疫输出格式差异。
/// - macOS：`lsof -nP -iTCP:<port> -sTCP:LISTEN`。
/// `pub(crate)`：supervisor 认领已在运行的 dsh 时复用（避免重复解析）。
pub(crate) fn listening_pid_on_port(port: u16) -> Option<u32> {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg("netstat -ano -p tcp");
        let out = capture(cmd)?;
        for line in out.lines() {
            if let Some(pid) = parse_listening_pid(line, port) {
                return Some(pid);
            }
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        listening_pid_linux(port)
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("lsof");
        cmd.args(["-nP", "-iTCP", &format!("{port}"), "-sTCP:LISTEN"]);
        let out = capture(cmd)?;
        for line in out.lines() {
            if let Some(pid) = parse_lsof_listener(line, port) {
                return Some(pid);
            }
        }
        None
    }
}

/// Linux 端口占用探测：直读 `/proc/net/tcp` 与 `/proc/net/tcp6`（审计 P2-2）。
///
/// 两步走：
/// 1. 在 TCP 表里找 `st == 0A`（LISTEN）且本地端口匹配的行，记下 socket inode；
/// 2. 遍历 `/proc/<pid>/fd/*` 的 readlink，命中 `socket:[<inode>]` 即得占用 pid。
///
/// 全程不 spawn 任何外部命令，`/proc` 缺失时（非 Linux 内核挂载）返回 None。
#[cfg(target_os = "linux")]
fn listening_pid_linux(port: u16) -> Option<u32> {
    let want_port = format!("{:04X}", port);
    let mut inodes: Vec<String> = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue; // tcp6 在部分内核不存在，跳过不影响
        };
        for line in text.lines().skip(1) {
            if let Some(inode) = tcp_line_inode(line, &want_port) {
                inodes.push(inode);
            }
        }
    }
    if inodes.is_empty() {
        return None;
    }
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return None;
    };
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let t = target.to_string_lossy();
            if let Some(num) = t.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) {
                if inodes.iter().any(|i| i == num) {
                    return Some(pid);
                }
            }
        }
    }
    None
}

/// 解析 `lsof -iTCP:<port>` 单行（macOS）：
/// `COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME`，NAME 列含 `*:3080 (LISTEN)`。
#[cfg(target_os = "macos")]
fn parse_lsof_listener(line: &str, port: u16) -> Option<u32> {
    if line.starts_with("COMMAND") {
        return None; // 表头
    }
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 2 {
        return None;
    }
    let name = f.last()?;
    // NAME 形如 `*:3080 (LISTEN)` / `127.0.0.1:3080 (LISTEN)`——末列含 (LISTEN) 且端口匹配
    if name.contains(&format!(":{port}")) && name.contains("(LISTEN)") {
        f.get(1)?.parse::<u32>().ok()
    } else {
        None
    }
}

/// 纯函数：解析单行 netstat 输出，命中端口则返回 PID（供单测 + supervisor 认领逻辑）。
/// `pub(crate)`：supervisor 认领逻辑复用同一解析规则。
/// Unix 分支不解析 netstat（用 ss/lsof），故非 Windows 下未引用——保留供测试/文档。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_listening_pid(line: &str, port: u16) -> Option<u32> {
    let f: Vec<&str> = line.split_whitespace().collect();
    // 形如：TCP    127.0.0.1:3080    0.0.0.0:0    LISTENING    1234
    if f.len() >= 5 && f[3].eq_ignore_ascii_case("LISTENING") {
        let local = f[1];
        let p = local.rsplit(':').next()?;
        if p.parse::<u16>().ok()? == port {
            return f[4].parse::<u32>().ok();
        }
    }
    None
}

/// 该 PID 的命令行是否指向「我们的 dsh」——决定能否在主治阶段就安全结束
fn is_our_process(pid: u32, cfg: &AppConfig) -> bool {
    let cl = pid_cmdline(pid).unwrap_or_default();
    cl.contains("dsh") || cl.contains(&cfg.port.to_string()) || cl.contains(".dsh")
}

/// 取某 PID 的命令行（查 ps_table；查不到返回 None）
fn pid_cmdline(pid: u32) -> Option<String> {
    ps_table()
        .into_iter()
        .find(|(p, _, _, _)| *p == pid)
        .map(|(_, _, _, cl)| cl)
}

/// 全进程表（pid, ppid, name, cmdline），平台化。
/// - Windows：PowerShell Get-CimInstance 一次取回（替代已弃用的 wmic）。
/// - Linux：直读 `/proc/<pid>/stat` + `cmdline`（见 `ps_table_linux`），
///   不依赖 ps——容器与精简系统常缺失，且 BSD/BusyBox 输出格式不同。
/// - macOS：`ps -axo pid,ppid,comm,command`（BSD 变体）。
fn ps_table() -> Vec<(u32, u32, String, String)> {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(
            "powershell -NoProfile -NonInteractive -Command \
             \"Get-CimInstance Win32_Process | ForEach-Object { \\\"$($_.ProcessId)|$($_.ParentProcessId)|$($_.Name)|$($_.CommandLine)\\\" }\"",
        );
        let Some(out) = capture(cmd) else { return vec![] };
        let mut res = Vec::new();
        for line in out.lines() {
            let mut f = line.splitn(4, '|');
            let (Some(pid), Some(ppid), Some(name)) = (
                f.next().and_then(|s| s.trim().parse::<u32>().ok()),
                f.next().and_then(|s| s.trim().parse::<u32>().ok()),
                f.next().map(|s| s.trim().to_string()),
            ) else {
                continue;
            };
            let cl = f.next().unwrap_or("").trim().to_string();
            res.push((pid, ppid, name, cl));
        }
        res
    }
    #[cfg(target_os = "linux")]
    {
        ps_table_linux()
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("ps");
        cmd.args(["-axo", "pid,ppid,comm,command"]);
        let Some(out) = capture(cmd) else { return vec![] };
        let mut res = Vec::new();
        for line in out.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 4 {
                continue;
            }
            let (Some(pid), Some(ppid)) = (
                f[0].trim().parse::<u32>().ok(),
                f[1].trim().parse::<u32>().ok(),
            ) else {
                continue;
            };
            let name = f[2].trim().to_string();
            // command 可能含空格：从第 4 列拼到行尾（跳过表头第一行）
            let cl = f[3..].join(" ");
            if name == "PID" {
                continue; // ps 表头
            }
            res.push((pid, ppid, name, cl));
        }
        res
    }
}

/// 解析 `/proc/net/tcp`(或 tcp6) 一行：仅当该行是 LISTEN(`st == 0A`) 且本地端口
/// 与目标一致时返回 socket inode（第 10 列），否则 None。纯函数，可单测。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn tcp_line_inode(line: &str, want_port: &str) -> Option<String> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.get(3) != Some(&"0A") {
        return None; // 0A = LISTEN
    }
    let local = f.get(1)?; // 形如 0100007F:0C08（little-endian IP + hex port）
    let (_, port_hex) = local.split_once(':')?;
    if !port_hex.eq_ignore_ascii_case(want_port) {
        return None;
    }
    f.get(9).map(|s| s.to_string())
}

/// 解析 `/proc/<pid>/stat` 一行 → (pid, ppid, comm)。
/// comm 本身可能含空格与括号（内核以 `(` 包裹）：取「第一个 `(` 之后」到
/// 「最后一个 `)` 之前」为 comm，`)` 之后才是 state/ppid 等字段。
/// 僵尸进程（state=Z）返回 None（无 cmdline 意义，且污染孤儿判断）。纯函数，可单测。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_stat(line: &str) -> Option<(u32, u32, String)> {
    let (head, _) = line.split_once(' ')?;
    let pid = head.parse::<u32>().ok()?;
    let lp = line.find('(')?;
    let rp = line.rfind(')')?;
    if rp <= lp {
        return None;
    }
    let comm = line[lp + 1..rp].to_string();
    let fields: Vec<&str> = line[rp + 1..].split_whitespace().collect();
    let state = fields.first().copied()?;
    if state == "Z" {
        return None;
    }
    let ppid = fields.get(1)?.parse::<u32>().ok()?;
    Some((pid, ppid, comm))
}

/// Linux 进程表：直读 `/proc/<pid>/stat`（pid/ppid/comm）与 `cmdline`（审计 P2-2）。
///
/// stat 行解析按内核约定：`pid (comm) state ppid ...`，comm 本身**可能含空格或括号**，
/// 因此取「第一个 `(` 之后」到「最后一个 `)` 之前」为 comm，`)` 之后才是状态字段。
/// 僵尸进程（state=Z）跳过：无 cmdline 意义，且会污染孤儿判断。
/// cmdline 为 NUL 分隔 argv，替换为空格。内核线程 cmdline 为空，comm 仍可读。
#[cfg(target_os = "linux")]
fn ps_table_linux() -> Vec<(u32, u32, String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            continue;
        };
        let Some((_pid, ppid, comm)) = parse_proc_stat(&stat) else {
            continue;
        };
        let cl = std::fs::read(e.path().join("cmdline"))
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
            .unwrap_or_default();
        out.push((pid, ppid, comm, cl.trim().to_string()));
    }
    out
}

/// node/dsh 相关进程名（与 dsh 引擎链相关；Windows 下 dsh.cmd 为 CLI 包装，node.exe 为实际运行器）
fn name_matches_dsh(name: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        matches!(name, "node.exe" | "dsh.exe" | "dsh.cmd")
    }
    #[cfg(not(target_os = "windows"))]
    {
        matches!(name, "node" | "dsh")
    }
}

/// 当前引擎进程树（supervisor 管理的顶层进程 + 其全部后代）。
/// 顶层 pid 来自 supervisor::status().pid（spawn 的 cmd.exe 树根）；无引擎（未启动/已退出）→ 空集。
/// 用途：诊疗/急救时排除活引擎，避免把正在运行的 dsh 当孤儿杀掉。
fn engine_tree_pids() -> std::collections::HashSet<u32> {
    let Some(root) = crate::supervisor::status().pid else {
        return std::collections::HashSet::new();
    };
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for (pid, ppid, _, _) in ps_table() {
        children.entry(ppid).or_default().push(pid);
    }
    collect_subtree(&children, root)
}

/// 纯函数：从进程父子表收集 root 的整棵子树（含 root）。供单测。
fn collect_subtree(
    children: &std::collections::HashMap<u32, Vec<u32>>,
    root: u32,
) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    let mut stack = vec![root];
    out.insert(root);
    while let Some(p) = stack.pop() {
        if let Some(cs) = children.get(&p) {
            for c in cs {
                if out.insert(*c) {
                    stack.push(*c);
                }
            }
        }
    }
    out
}

// ===================== patch 文件解析（轻量，不引 YAML 库） =====================

/// 返回所有「file:// 源指向不存在路径」的条目 (id, src)。
///
/// 解析与路径归一化都由 `patchyml` 提供——与 `status.rs` 共用同一套规则，
/// 避免「自愈认为该删、管理页却看不到这个条目」这类矛盾判断。
fn orphan_file_entries(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in parse_entries(text) {
        let id = match entry.id {
            Some(i) => i,
            None => continue,
        };
        if let Some(path) = file_uri_path(&entry.text) {
            if !path.is_empty() && !Path::new(&path).exists() {
                out.push((id, path));
            }
        }
    }
    out
}

// `remove_entry` / `looks_like_patch` 已上移到 `patchyml`，与 status.rs 共用同一实现。
// 此前两份手写解析器规则不同（嵌套判定、id 写法识别），会对同一文件给出矛盾结论。

/// 收集 .dsh 下的残缺下载/临时文件（有界扫描，防止大目录卡死）
fn collect_junk(root: &Path, max: usize) -> Vec<PathBuf> {
    fn is_junk_name(name: &str) -> bool {
        name.ends_with(".tmp")
            || name.ends_with(".partial")
            || name.ends_with(".crdownload")
            || name == "_downloads"
            || name == "partial"
    }
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, max: usize) {
        if depth == 0 || out.len() >= max {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if is_junk_name(&name) {
                out.push(p);
                if out.len() >= max {
                    return;
                }
                continue;
            }
            if p.is_dir() {
                walk(&p, depth - 1, out, max);
                if out.len() >= max {
                    return;
                }
            }
        }
    }
    let mut out = Vec::new();
    if root.is_dir() {
        walk(root, 5, &mut out, max);
    }
    out
}

// ===================== 单测 =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_escalation_ladder() {
        assert_eq!(Mode::Inspect.escalate(), Mode::Treat);
        assert_eq!(Mode::Treat.escalate(), Mode::Attend);
        assert_eq!(Mode::Attend.escalate(), Mode::Emergency);
        assert_eq!(Mode::Emergency.escalate(), Mode::Emergency);
    }

    #[test]
    fn mode_from_str_variants() {
        assert_eq!(Mode::from_str("inspect"), Some(Mode::Inspect));
        assert_eq!(Mode::from_str("处置"), Some(Mode::Treat));
        assert_eq!(Mode::from_str("ATTEND"), Some(Mode::Attend));
        assert_eq!(Mode::from_str("急救"), Some(Mode::Emergency));
        assert_eq!(Mode::from_str("nonsense"), None);
    }

    #[test]
    fn mode_for_restart_progression() {
        assert_eq!(Mode::for_restart(1), Mode::Treat);
        assert_eq!(Mode::for_restart(2), Mode::Attend);
        assert_eq!(Mode::for_restart(3), Mode::Emergency);
        assert_eq!(Mode::for_restart(99), Mode::Emergency);
    }

    #[test]
    fn auto_authorization_matrix() {
        // 绿：除巡检外都自动
        assert!(should_auto(Blast::Green, Mode::Treat));
        assert!(!should_auto(Blast::Green, Mode::Inspect));
        // 黄：主治及以上
        assert!(should_auto(Blast::Yellow, Mode::Attend));
        assert!(should_auto(Blast::Yellow, Mode::Emergency));
        assert!(!should_auto(Blast::Yellow, Mode::Treat));
        // 红：仅急救
        assert!(should_auto(Blast::Red, Mode::Emergency));
        assert!(!should_auto(Blast::Red, Mode::Attend));
    }

    #[test]
    fn netstat_line_parsing() {
        let line = "  TCP    127.0.0.1:3080    0.0.0.0:0    LISTENING    1234";
        assert_eq!(parse_listening_pid(line, 3080), Some(1234));
        assert_eq!(parse_listening_pid(line, 3081), None);
        let all = "  TCP    0.0.0.0:0    LISTENING    9";
        assert_eq!(parse_listening_pid(all, 3080), None);
    }

    // ---------- P2-2：/proc 直读的解析纯函数（Linux 集成靠 /proc 本身，解析逻辑跨平台可测） ----------

    /// tcp 表行：LISTEN(0A) 且端口匹配 → inode；端口不匹配 / 非 LISTEN / 头行 → None。
    #[test]
    fn tcp_line_inode_parsing() {
        // 列序：sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
        let listen_3080 =
            "   0: 0100007F:0C08 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 123456 1 4";
        assert_eq!(
            tcp_line_inode(listen_3080, "0C08").as_deref(),
            Some("123456"),
            "LISTEN + 端口 3080 应命中 inode"
        );
        assert_eq!(tcp_line_inode(listen_3080, "0C09"), None, "端口不符应跳过");
        assert_eq!(tcp_line_inode(listen_3080, "0c08").as_deref(), Some("123456"), "端口 hex 大小写不敏感");

        // st = 01（ESTABLISHED）不是 LISTEN
        let established =
            "   1: 0100007F:0C08 0100007F:1F90 01 00000000:00000000 00:00000000 00000000     0        0 654321 1 4";
        assert_eq!(tcp_line_inode(established, "0C08"), None, "非 LISTEN 不应命中");

        // 表头行
        assert_eq!(tcp_line_inode("  sl  local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode", "0C08"), None);
    }

    /// stat 行解析：常规、comm 含空格/括号、僵尸进程三类边界。
    #[test]
    fn proc_stat_parsing() {
        // 常规：pid 1234, comm node, state S, ppid 1
        let plain = "1234 (node) S 1 1234 1234 0 -1 4194560 123 0 0 0 0 0 0 0 20 0 1 0";
        let (pid, ppid, comm) = parse_proc_stat(plain).unwrap();
        assert_eq!((pid, ppid), (1234, 1));
        assert_eq!(comm, "node");

        // comm 含空格：`/usr/bin/foo bar` 之类（真实 comm 允许空格）
        let spaced = "5678 (my proc) S 2 5678 5678 0 -1 4194560 123 0 0 0 0 0 0 0 20 0 1 0";
        let (pid, ppid, comm) = parse_proc_stat(spaced).unwrap();
        assert_eq!((pid, ppid), (5678, 2));
        assert_eq!(comm, "my proc", "comm 含空格必须整体保留");

        // 僵尸：state Z → None（跳过，避免污染孤儿判断）
        let zombie = "9999 (defunct) Z 1 9999 9999 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_proc_stat(zombie), None, "僵尸进程应被跳过");

        // 畸形行
        assert_eq!(parse_proc_stat("not a stat line"), None);
    }

    #[test]
    fn patch_entry_removal() {
        let patch = "- id: a\n  config: {}\n- id: b\n  src: file:///gone\n- id: c\n  x: 1\n";
        let removed = remove_entry(patch, "b").unwrap();
        assert!(!removed.contains("id: b"));
        assert!(removed.contains("id: a"));
        assert!(removed.contains("id: c"));
        assert_eq!(remove_entry(patch, "zzz"), None);
    }

    #[test]
    fn detects_orphan_file_entry() {
        // 存在路径用「扫描得到的临时目录」推导（不写死绝对地址）；不存在路径用于验孤儿
        let tmp = std::env::temp_dir();
        let tmp_uri = format!("file:///{}", tmp.display().to_string().replace('\\', "/"));
        let patch = format!(
            "- id: gone\n  src: file:///C:/no/such/dir/here-xyz\n- id: ok\n  src: {tmp_uri}\n"
        );
        let orphans = orphan_file_entries(&patch);
        let ids: Vec<_> = orphans.iter().map(|(i, _)| i.clone()).collect();
        // 不存在的路径 → 算孤儿
        assert!(ids.contains(&"gone".to_string()));
        // 临时目录必存在 → 不算孤儿（空临时目录则跳过该断言，避免平台差异误判）
        if !tmp.as_os_str().is_empty() {
            assert!(!ids.contains(&"ok".to_string()));
        }
    }

    #[test]
    fn patch_shape_sanity() {
        assert!(looks_like_patch("- id: a\n  x: 1\n"));
        assert!(looks_like_patch("# only comment\n"));
        assert!(looks_like_patch("[]\n"));
        assert!(!looks_like_patch("this is not a list at top level: oops\n"));
    }

    #[test]
    fn subtree_collects_all_descendants() {
        use std::collections::{HashMap, HashSet};
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        children.insert(100, vec![101, 102]);
        children.insert(101, vec![103]);
        children.insert(103, vec![104]);
        // 102 无子；105 是别的树的
        children.insert(999, vec![105]);
        let tree = collect_subtree(&children, 100);
        assert_eq!(tree, HashSet::from([100, 101, 102, 103, 104]));
        assert!(!tree.contains(&105), "不应混入其他树");
        // 无子树的叶子
        assert_eq!(collect_subtree(&children, 105), HashSet::from([105]));
        // 不在表中的根 → 只有自己
        assert_eq!(collect_subtree(&children, 777), HashSet::from([777]));
    }

    #[test]
    fn dsh_process_name_matching() {
        // 平台相关进程名：Windows 带 .exe/.cmd 后缀，Unix 无后缀
        #[cfg(target_os = "windows")]
        {
            assert!(name_matches_dsh("node.exe"));
            assert!(name_matches_dsh("dsh.exe"));
            assert!(name_matches_dsh("dsh.cmd"));
            assert!(!name_matches_dsh("cmd.exe"));
            assert!(!name_matches_dsh("powershell.exe"));
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert!(name_matches_dsh("node"));
            assert!(name_matches_dsh("dsh"));
            assert!(!name_matches_dsh("node.exe")); // Unix 只认无后缀名
            assert!(!name_matches_dsh("cmd.exe"));
            assert!(!name_matches_dsh("powershell.exe"));
        }
        assert!(!name_matches_dsh(""));
    }
}
