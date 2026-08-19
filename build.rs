fn main() {
    // 仅 Windows 嵌入资源（图标）
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // embed-resource 3.x：compile 返回 must_use 的 CompilationResult（失败会自身报错），
        // 需要显式消费。icon.ico 是提交的静态资产（resources/icon.ico）。
        let _ = embed_resource::compile("resources/icon.rc", embed_resource::NONE);
    }
}
