//! 极简 i18n（方向 v4 P0，2026-08-27）：config.lang 决定语言，`tr(zh, en)` 就地选择文案。
//!
//! 语言在**首次调用时缓存**（config.json 改动需重启生效，与 config.rs 注释一致），
//! 避免托盘 1s 刷新时反复读盘。范围约定（方向 v4 拍板）：壳内代码面字符串
//! （托盘/通知/CLI/管理页 API 消息）；插件 UI 由插件项目自行处理；日志行保持中文
//! （诊断信息，非用户界面文案）。

use std::sync::{Mutex, OnceLock};

static LANG: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// 当前语言（zh / en；缓存于首次调用，重启生效）
pub fn lang() -> String {
    let m = LANG.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = m.lock() {
        if g.is_none() {
            *g = Some(crate::config::load().lang.clone());
        }
        if let Some(l) = g.as_ref() {
            return l.clone();
        }
    }
    "zh".to_string()
}

/// 是否英文界面（format 分支用，避免把数字拼进 tr 的静态字符串）
pub fn is_en() -> bool {
    lang() == "en"
}

/// 就地双语选择：zh 为默认，lang=en 时返回 en。参数必须是 'static 字符串字面量。
pub fn tr(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() {
        en
    } else {
        zh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tr_selects_zh_by_default() {
        // 默认 zh（测试环境 config 不存在 → default zh）
        assert_eq!(tr("中文", "English"), "中文");
    }
}
