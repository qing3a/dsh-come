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
                evidence: format!("HTTP 200 且 netstat 显示 127.0.0.1:{} LISTENING（PID {pid}）", cfg.port),
                blast: Blast::Green,
                remedy: None, // start() 的认领逻辑处理，无需处置
            });
        } else {
            out.push(Finding {
                id: "port-conflict",
                title: format!("端口 {} 被不健康进程占用，dsh web 无法绑定", cfg.port),
                evidence: format!("netstat 显示 127.0.0.1:{} 已被 PID {} 监听（LISTENING），但 HTTP 不响应——可能是僵尸 node 残留", cfg.port, pid),
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
    // 两处根都扫：dsh 数据根（.dsh）与启动器根（dsh-desktop），去重合并成一条发现
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
/// 3) 同样扫描启动器根目录（部分布局 patch 可能在 dsh-desktop 下）
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

/// 捕获命令 stdout（Windows 下经 cmd /C；隐藏窗口）
fn capture(mut cmd: Command) -> Option<String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    crate::supervisor::hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 查端口被谁监听（netstat -ano -p tcp），返回占用 PID
/// `pub(crate)`：supervisor 认领已在运行的 dsh 时复用（避免重复 netstat 解析）
pub(crate) fn listening_pid_on_port(port: u16) -> Option<u32> {
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

/// 纯函数：解析单行 netstat 输出，命中端口则返回 PID（供单测）
/// `pub(crate)`：supervisor 认领逻辑复用同一解析规则
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

/// 全进程表（pid, ppid, name, cmdline）：PowerShell Get-CimInstance 一次取回。
/// 替代 wmic（Win11 24H2+ 已弃用，部分新机器无此命令）；PowerShell 5.1 全系可用。
fn ps_table() -> Vec<(u32, u32, String, String)> {
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

/// node/dsh 相关进程名（与 dsh 引擎链相关；dsh.cmd 为 CLI 包装，node.exe 为实际运行器）
fn name_matches_dsh(name: &str) -> bool {
    matches!(name, "node.exe" | "dsh.exe" | "dsh.cmd")
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

/// 顶层列表项（行首 `- `，无前导空白）的行区间 [start, end)
fn top_level_entry_ranges(text: &str) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, ln)| ln.starts_with("- "))
        .map(|(i, _)| i)
        .collect();
    let mut ranges = Vec::new();
    for (k, s) in starts.iter().enumerate() {
        let e = if k + 1 < starts.len() { starts[k + 1] } else { lines.len() };
        ranges.push((*s, e));
    }
    ranges
}

fn entry_id_of(block: &str) -> Option<String> {
    for ln in block.lines() {
        let t = ln.trim_start();
        let t = t.strip_prefix("- ").unwrap_or(t); // 顶层列表项首行带 "- "
        if let Some(rest) = t.strip_prefix("id:") {
            let v = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// 返回所有「file:// 源指向不存在路径」的条目 (id, src)
fn orphan_file_entries(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let ranges = top_level_entry_ranges(text);
    let mut out = Vec::new();
    for (s, e) in ranges {
        let block: String = lines[s..e].join("\n");
        let id = match entry_id_of(&block) {
            Some(i) => i,
            None => continue,
        };
        if let Some((idx, _)) = block.match_indices("file://").next() {
            let rest = &block[idx + "file://".len()..];
            let raw: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
                .collect();
            // 归一化 file:///C:/x（三重斜杠）→ C:/x（Windows 下 /C: 不会被正确解析，
            // 否则真实存在的路径会被误判为孤儿）
            let path = if let Some(stripped) = raw.strip_prefix('/') {
                if stripped.as_bytes().get(1) == Some(&b':') {
                    stripped.to_string() // /C:/x → C:/x
                } else {
                    raw
                }
            } else {
                raw
            };
            if !path.is_empty() && !Path::new(&path).exists() {
                out.push((id, path));
            }
        }
    }
    out
}

/// 从 patch 文本中删除指定 id 的顶层条目，返回新文本；找不到返回 None
fn remove_entry(text: &str, entry_id: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let ranges = top_level_entry_ranges(text);
    for (s, e) in &ranges {
        let block: String = lines[*s..*e].join("\n");
        if entry_id_of(&block).as_deref() == Some(entry_id) {
            let mut new_lines = lines[..*s].to_vec();
            new_lines.extend_from_slice(&lines[*e..]);
            return Some(new_lines.join("\n"));
        }
    }
    None
}

/// 轻量判断文本是否像合法 patch 列表（不追求完整 YAML 解析）
fn looks_like_patch(text: &str) -> bool {
    for ln in text.lines() {
        let t = ln.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "[]" || t == "{}" {
            continue; // 空序列 / 空映射，合法
        }
        if t.starts_with("- ") {
            continue; // 顶层列表项
        }
        if ln.starts_with(' ') {
            continue; // 缩进续行
        }
        // 顶层出现非列表/非注释/非缩进的行 → 可疑
        return false;
    }
    true
}

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
        assert!(name_matches_dsh("node.exe"));
        assert!(name_matches_dsh("dsh.exe"));
        assert!(name_matches_dsh("dsh.cmd"));
        assert!(!name_matches_dsh("cmd.exe"));
        assert!(!name_matches_dsh("powershell.exe"));
        assert!(!name_matches_dsh(""));
    }
}
