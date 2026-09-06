# -*- coding: utf-8 -*-
"""移除 Cargo.toml 中的 image 依赖 + 删除鲸鱼托盘 PNG 资源。"""
import io
import os

# 1. Cargo.toml 移除 image 依赖
p = 'Cargo.toml'
with io.open(p, encoding='utf-8') as f:
    c = f.read()
old = '# 托盘图标 PNG 解码 + 缩放（仅 png feature，最小依赖）\nimage = { version = "0.25", default-features = false, features = ["png"] }\n'
if old in c:
    c = c.replace(old, '')
    with io.open(p, 'w', encoding='utf-8', newline='') as f:
        f.write(c)
    print('[OK] Cargo.toml image dependency removed')
else:
    print('[WARN] image dependency line not found in Cargo.toml')

# 2. 删除鲸鱼托盘 PNG（已不再被引用）
for fname in ['resources/tray-light.png', 'resources/tray-dark.png']:
    if os.path.exists(fname):
        os.remove(fname)
        print(f'[OK] deleted {fname}')
    else:
        print(f'[SKIP] {fname} not found')

# 3. 删除临时修复脚本自身（任务完成后清理）
#    保留在 scripts/ 作为记录也可以，但作为一次性修复脚本更合适删除。
self_script = 'scripts/fix_tray_icon.py'
if os.path.exists(self_script):
    os.remove(self_script)
    print(f'[OK] removed temp script {self_script}')
print('done')
