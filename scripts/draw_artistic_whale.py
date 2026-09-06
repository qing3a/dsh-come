from PIL import Image, ImageDraw
import math

# 画布
W, H = 1024, 1024

# 1. 背景：极浅蓝
bg_color = (232, 244, 252)  # #e8f4fc
img = Image.new('RGB', (W, H), bg_color)
draw = ImageDraw.Draw(img)

# 2. 四条天蓝宽竖线
stripe_color = (125, 211, 252)  # #7dd3fc
line_w = 130
gap = (W - 4 * line_w) // 3
for i in range(4):
    x0 = i * (line_w + gap)
    x1 = x0 + line_w
    draw.rectangle([x0, 0, x1, H], fill=stripe_color)

# 3. 画艺术化鲸鱼剪影（宝蓝，上仰姿态，流畅曲线，无写实细节）
whale_color = (37, 99, 235)  # #2563eb

# 用参数方程生成流畅的鲸鱼轮廓
# 鲸鱼中心位置和姿态
cx, cy = 480, 520  # 中心
angle = -15  # 上仰角度（度）

# 生成鲸鱼轮廓点（艺术化：流畅的梭形身体 + 简洁V形尾 + 小三角鳍）
def rotate_point(x, y, cx, cy, angle_deg):
    rad = math.radians(angle_deg)
    cos_a, sin_a = math.cos(rad), math.sin(rad)
    dx, dy = x - cx, y - cy
    return (cx + dx * cos_a - dy * sin_a,
            cy + dx * sin_a + dy * cos_a)

# 身体轮廓：上弧线和下弧线（艺术化，流畅）
body_points = []
# 上轮廓：从头部到尾部
n_pts = 40
for i in range(n_pts + 1):
    t = i / n_pts
    # x: 从头部(左)到尾部(右)
    x = cx - 280 + t * 520
    # 上轮廓 y: 流畅的弧线
    y = cy - 90 * math.sin(math.pi * t) * (1 - 0.3 * t) - 20 * t
    body_points.append((x, y))

# 尾鳍上叶
tail_top = [(cx + 240, cy - 30), (cx + 340, cy - 110), (cx + 310, cy - 20)]
body_points.extend(tail_top)

# 尾鳍下叶
tail_bottom = [(cx + 310, cy + 20), (cx + 340, cy + 100), (cx + 240, cy + 40)]
body_points.extend(tail_bottom)

# 下轮廓：从尾部到头部
for i in range(n_pts + 1):
    t = 1 - i / n_pts
    x = cx - 280 + t * 520
    # 下轮廓 y: 流畅的弧线（腹部稍圆）
    y = cy + 100 * math.sin(math.pi * t) * (1 - 0.2 * t) + 10 * t
    body_points.append((x, y))

# 头部圆弧（闭合）
head_pts = [(cx - 280, cy + 30), (cx - 310, cy + 10), (cx - 300, cy - 20), (cx - 280, cy - 30)]
body_points.extend(head_pts)

# 旋转所有点（上仰姿态）
rotated = [rotate_point(x, y, cx, cy, angle) for x, y in body_points]

# 画鲸鱼身体
draw.polygon(rotated, fill=whale_color)

# 画胸鳍（小三角形，艺术化）
fin_points = [
    rotate_point(cx - 50, cy + 60, cx, cy, angle),
    rotate_point(cx + 30, cy + 130, cx, cy, angle),
    rotate_point(cx + 10, cy + 50, cx, cy, angle),
]
draw.polygon(fin_points, fill=whale_color)

# 画背鳍（小三角形）
dorsal_points = [
    rotate_point(cx + 80, cy - 80, cx, cy, angle),
    rotate_point(cx + 130, cy - 140, cx, cy, angle),
    rotate_point(cx + 140, cy - 70, cx, cy, angle),
]
draw.polygon(dorsal_points, fill=whale_color)

# 4. 圆角蒙版（圆角方形）
mask = Image.new('L', (W, H), 0)
mask_draw = ImageDraw.Draw(mask)
radius = 120
mask_draw.rounded_rectangle([0, 0, W-1, H-1], radius=radius, fill=255)

# 应用圆角（圆角外用白色）
result = Image.new('RGB', (W, H), (255, 255, 255))
result.paste(img, (0, 0), mask)

result.save('app_icon_artistic.png')
print('saved app_icon_artistic.png')
