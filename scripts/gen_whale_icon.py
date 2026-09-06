"""将鲸鱼源图生成多尺寸 ICO（应用图标+托盘图标同源）及各尺寸 PNG。"""
from PIL import Image
import os

PROJECT = r"C:\Users\Administrator\Desktop\dsh-come"
SRC = os.path.join(PROJECT, "whale_icon_src.png")
ICO_OUT = os.path.join(PROJECT, "resources", "icon.ico")

# 标准 ICO 尺寸（从小到大，256 用 PNG 压缩，Vista+ 支持）
ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]

# 同时导出的 PNG 尺寸
PNG_SIZES = [256, 512, 1024]

img = Image.open(SRC)
print(f"源图: {img.size}, mode={img.mode}")

# 确保 RGBA（ICO 需要）
if img.mode != "RGBA":
    img = img.convert("RGBA")

# 生成 ICO（包含多尺寸帧）
img.save(ICO_OUT, format="ICO", sizes=ICO_SIZES)
print(f"已生成 ICO: {ICO_OUT} ({os.path.getsize(ICO_OUT)} bytes)")

# 验证 ICO 内容
from PIL import IcoImagePlugin
ico = Image.open(ICO_OUT)
print(f"ICO 帧尺寸: {ico.ico.sizes() if hasattr(ico, 'ico') else 'n/a'}")

# 导出各尺寸 PNG
for size in PNG_SIZES:
    resized = img.resize((size, size), Image.LANCZOS)
    out = os.path.join(PROJECT, f"icon_{size}.png")
    resized.save(out, format="PNG")
    print(f"已生成: {out} ({os.path.getsize(out)} bytes)")

print("完成")
