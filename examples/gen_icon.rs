//! 一次性生成 `resources/icon.ico`（官方 favicon 多尺寸 PNG 条目，Vista+ 支持 PNG 压缩条目）。
//!
//! 用法：`cargo run --example gen_icon`（需要 dev-dependencies 的 png crate）
//! 产物提交进 git；build.rs 用 embed-resource 把 icon.ico 嵌入 exe。
//! 图标为 DeepSeek AI 商标（官方 favicon 资产），仅作标识引用。

use std::path::Path;

fn main() {
    let sizes = [16u32, 32, 48, 64, 128, 256];
    let mut pngs: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    for &s in &sizes {
        let pm = render(s);
        pngs.push((s, s, encode_png(&pm, s, s)));
    }

    // ICO 容器：ICONDIR(6 字节) + ICONDIRENTRY(16 字节 × n) + PNG 数据
    // 注意：ICONDIR 三个字段（reserved/type/count）都是 little-endian——
    // count 低字节在前（[len, 0]），写反会让 Windows 解析出 1536 个条目 → exe 图标显示为默认图标
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0, pngs.len() as u8, 0]); // reserved, type=1(icon), count(LE)
    let mut offset = (6 + 16 * pngs.len()) as u32;
    for (w, h, data) in &pngs {
        // width/height：0 表示 256
        out.extend_from_slice(&[(*w % 256) as u8, (*h % 256) as u8, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, _, data) in &pngs {
        out.extend_from_slice(data);
    }

    let path = Path::new("resources").join("icon.ico");
    std::fs::create_dir_all("resources").unwrap();
    std::fs::write(&path, &out).unwrap();
    println!("written {} ({} bytes, {} sizes)", path.display(), out.len(), pngs.len());
}

fn render(size: u32) -> resvg::tiny_skia::Pixmap {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg::{Options, Tree};
    // exe 图标合成：深蓝圆角方块 + 白色官方鲸鱼。
    // 原 favicon 是黑鲸鱼 + 透明背景——深色主题（资源管理器深色模式）下黑鲸鱼隐形，
    // 用户会以为「没有图标」；加品牌底色后任意背景都可见。
    const SVG: &str = include_str!("../assets/favicon.svg");
    let d = extract_path_d(SVG);
    let composed = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 50 50"><rect width="50" height="50" rx="11" fill="#224AA0"/><path d="{d}" fill="#ffffff"/></svg>"##,
        s = size
    );
    let tree = Tree::from_str(&composed, &Options::default()).unwrap();
    let mut pm = Pixmap::new(size, size).unwrap();
    resvg::render(&tree, Transform::default(), &mut pm.as_mut());
    pm
}

/// 提取 favicon.svg 中 path 的 d 属性值（合成图标复用官方鲸鱼轮廓）
fn extract_path_d(svg: &str) -> &str {
    let marker = r#" d=""#;
    let start = svg.find(marker).map(|i| i + marker.len() - 1).expect("favicon.svg 应有 d 属性");
    let after = &svg[start + 1..];
    let end = after.find('"').expect("d 属性值应有收尾引号");
    &after[..end]
}

fn encode_png(pm: &resvg::tiny_skia::Pixmap, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(pm.data()).unwrap();
    }
    out
}
