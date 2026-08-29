//! `cordis.patch.yml` 的统一解析与保序编辑。
//!
//! # 为什么手写而不用 serde_yaml
//!
//! 曾评估引入 `serde_yaml` 做「真解析 + 保序编辑」，结论是**不行**：
//! 1. 该文件头部注释写明「`!!js` expressions allowed」——条目里可能出现 `!!js` 标签，
//!    serde_yaml 遇到未知 tag 会直接解析失败，而我们需要的是「看不懂也能安全跳过」。
//! 2. 反序列化再序列化会吃掉用户手写的注释与空行。本文件是**用户可编辑的配置**，
//!    自愈动它时必须保持其余字节原样，否则等于毁掉用户的注释。
//!
//! 所以这里保留行级手写解析，但**只写一份**——此前 `doctor.rs` 与 `status.rs`
//! 各有一套，判定规则不同（见 `parse_entries` 的注释），会对同一文件给出矛盾结论。
//!
//! # 统一的判定规则
//!
//! - **顶层条目**：行首（无缩进）以 `- ` 开头的行；其内容延伸到下一个顶层条目或文件末尾。
//!   嵌套列表（有缩进的 `- `）属于父条目的一部分，**不是**顶层条目。
//! - **字段**：在条目范围内按行查找 `key:`，允许 `- id: x` 与换行后 `  id: x` 两种写法，
//!   取首次出现，并去掉包裹的引号。

/// 一个顶层 patch 条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchEntry {
    /// `id:` 字段值（条目身份；自愈按 id 定位）
    pub id: Option<String>,
    /// `name:` 字段值（可能是 `file://…` 本地硬加载源）
    pub name: Option<String>,
    /// 行区间 `[start, end)`，供保序删除用
    pub start: usize,
    pub end: usize,
    /// 条目原文（用于 `file://` 等进一步提取，避免调用方再切一次）
    pub text: String,
}

/// 切分顶层条目。
///
/// 与旧的 `status.rs` 版本的关键差别：旧版用 `line.trim().strip_prefix("- id:")`，
/// 会把**嵌套**列表项也当成顶层条目；且只认 `- id:` 紧邻写法，`- name:` 在前、
/// `id:` 换行的写法会被整个漏掉——于是管理页看不到该插件，doctor 却能清理它。
pub fn parse_entries(text: &str) -> Vec<PatchEntry> {
    let lines: Vec<&str> = text.lines().collect();
    let mut starts: Vec<usize> = Vec::new();
    for (i, ln) in lines.iter().enumerate() {
        // 只认**行首**的 "- "：有缩进的是嵌套内容，属于上一个顶层条目
        if ln.starts_with("- ") {
            starts.push(i);
        }
    }

    let mut out = Vec::new();
    for (k, s) in starts.iter().enumerate() {
        let e = if k + 1 < starts.len() {
            starts[k + 1]
        } else {
            lines.len()
        };
        let block = lines[*s..e].join("\n");
        out.push(PatchEntry {
            id: field_value(&block, "id"),
            name: field_value(&block, "name"),
            start: *s,
            end: e,
            text: block,
        });
    }
    out
}

/// 在条目块内取标量字段值：忽略 `- ` 前缀与缩进，去掉包裹引号，取首次出现。
///
/// 用 `trim()` 后比对 `key:` 前缀，因此 `display_name:` 之类不会被误配成 `name:`
///（前缀比对要求行以 `name:` 开头）。
fn field_value(block: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for ln in block.lines() {
        let t = ln.trim();
        let t = t.strip_prefix("- ").unwrap_or(t);
        if let Some(rest) = t.strip_prefix(&prefix) {
            let v = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 删除指定 id 的顶层条目，返回新文本；找不到返回 `None`。
/// 只动目标条目的行区间，其余行（含注释与空行）原样保留。
pub fn remove_entry(text: &str, entry_id: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for e in parse_entries(text) {
        if e.id.as_deref() == Some(entry_id) {
            let mut new_lines = lines[..e.start].to_vec();
            new_lines.extend_from_slice(&lines[e.end..]);
            return Some(new_lines.join("\n"));
        }
    }
    None
}

/// 轻量判断文本是否像合法 patch 列表（不追求完整 YAML 解析）。
/// 顶层只允许：空行、注释、`[]`/`{}`、`- ` 列表项、缩进续行。
pub fn looks_like_patch(text: &str) -> bool {
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

/// 从条目原文里提取 `file://` 指向的路径。
///
/// 归一化 `file:///C:/x`（三重斜杠）→ `C:/x`：Windows 下 `/C:` 无法被正确解析，
/// 不归一化的话真实存在的路径会被误判成孤儿条目而被误删。
pub fn file_uri_path(text: &str) -> Option<String> {
    let idx = text.find("file://")?;
    let rest = &text[idx + "file://".len()..];
    let raw: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
        .collect();
    if raw.is_empty() {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix('/') {
        if stripped.as_bytes().get(1) == Some(&b':') {
            return Some(stripped.to_string()); // /C:/x → C:/x
        }
    }
    Some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两种 id 写法（紧邻 `- id:` / 换行后 `id:`）都必须识别——
    /// 这正是旧两套解析器分歧的地方。
    #[test]
    fn parses_both_id_layouts() {
        let text = "- id: aaa\n  name: 'file:///C:/x'\n- name: 'file:///D:/y'\n  id: bbb\n";
        let es = parse_entries(text);
        assert_eq!(es.len(), 2, "应切出两个顶层条目");
        assert_eq!(es[0].id.as_deref(), Some("aaa"));
        assert_eq!(es[1].id.as_deref(), Some("bbb"), "换行写法的 id 必须也能识别");
        assert!(es[1].name.as_deref().unwrap().contains("D:/y"));
    }

    /// 嵌套列表项不得被当成顶层条目（status 旧版会误判）。
    #[test]
    fn nested_items_are_not_top_level() {
        let text = "- id: parent\n  children:\n    - id: child\n      name: n\n";
        let es = parse_entries(text);
        assert_eq!(es.len(), 1, "嵌套的 '- id: child' 不应算顶层条目");
        assert_eq!(es[0].id.as_deref(), Some("parent"));
        // 取字段时也应命中父条目的 id，而非嵌套的
        assert_ne!(es[0].id.as_deref(), Some("child"));
    }

    /// 引号、CRLF、注释不应影响解析。
    #[test]
    fn tolerates_quotes_crlf_and_comments() {
        let text = "# 顶部注释\r\n- id: \"quoted\"\r\n  name: 'single'\r\n";
        let es = parse_entries(text);
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].id.as_deref(), Some("quoted"), "应去掉双引号");
        assert_eq!(es[0].name.as_deref(), Some("single"), "应去掉单引号");
    }

    /// 删除条目只动目标区间，注释与其余条目原样保留（保序编辑的核心保证）。
    #[test]
    fn remove_entry_preserves_the_rest() {
        let text = "# keep me\n- id: a\n  name: x\n- id: b\n  name: y\n";
        let out = remove_entry(text, "b").expect("应找到 id=b");
        assert!(!out.contains("id: b"));
        assert!(out.contains("# keep me"), "注释必须保留");
        assert!(out.contains("id: a"), "其余条目必须保留");
    }

    #[test]
    fn remove_entry_missing_returns_none() {
        assert_eq!(remove_entry("- id: a\n", "nope"), None);
    }

    /// file:// 路径归一化：/C:/x → C:/x，非盘符路径保持原样。
    #[test]
    fn file_uri_path_normalization() {
        assert_eq!(file_uri_path("name: 'file:///C:/a/b'").as_deref(), Some("C:/a/b"));
        assert_eq!(file_uri_path("name: 'file:///home/u'").as_deref(), Some("/home/u"));
        assert_eq!(file_uri_path("name: 'npm:foo'"), None);
        assert_eq!(file_uri_path(""), None);
    }

    #[test]
    fn patch_shape_sanity() {
        assert!(looks_like_patch("[]\n"));
        assert!(looks_like_patch("# c\n- id: a\n  x: 1\n"));
        assert!(!looks_like_patch("just-a-string\n"), "顶层裸字符串不是合法 patch");
    }
}
