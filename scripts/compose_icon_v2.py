from PIL import Image, ImageDraw
import math

# 1. 打开原图
img = Image.open('whale_new_pose.png').convert('RGB')
w, h = img.size
pixels = img.load()

# 2. 提取圆角蒙版（白色圆角外）
corner_mask = Image.new('L', (w, h), 0)
cm = corner_mask.load()
for y in range(h):
    for x in range(w):
        r, g, b = pixels[x, y]
        if r > 245 and g > 245 and b > 245:
            cm[x, y] = 0
        else:
            cm[x, y] = 255

# 3. 抠出鲸鱼：白色腹部(亮度>240) + 深蓝背部(亮度<100且B>R)
whale_mask = Image.new('L', (w, h), 0)
wm = whale_mask.load()
bg_color = (160, 210, 252)
for y in range(h):
    for x in range(w):
        if cm[x, y] == 0:
            continue
        r, g, b = pixels[x, y]
        gray = 0.299 * r + 0.587 * g + 0.114 * b
        # 白色腹部
        if gray > 235:
            wm[x, y] = 255
        # 深蓝背部
        elif gray < 110 and b > r + 20:
            wm[x, y] = 255
        # 渐变中间色（与背景距离大）
        else:
            dist = math.sqrt((r-bg_color[0])**2 + (g-bg_color[1])**2 + (b-bg_color[2])**2)
            if dist > 130:
                wm[x, y] = 255

# 中值滤波去噪 + 膨胀后腐蚀（闭运算，填补小空洞）
from PIL import ImageFilter
whale_mask = whale_mask.filter(ImageFilter.MedianFilter(size=5))
whale_mask = whale_mask.filter(ImageFilter.MaxFilter(size=3))  # 膨胀
whale_mask = whale_mask.filter(ImageFilter.MinFilter(size=3))  # 腐蚀

wm_data = whale_mask.load()
whale_count = sum(1 for y in range(h) for x in range(w) if wm_data[x, y] == 255)
print(f'whale pixels: {whale_count}')

# 4. 创建新背景 + 恰好4条短竖线
new_bg = (160, 210, 252)
new_img = Image.new('RGB', (w, h), (255, 255, 255))
draw = ImageDraw.Draw(new_img)
draw.rectangle([0, 0, w-1, h-1], fill=new_bg)

# 4条竖线：宽度90，长度占中间65%，左右各留80px边距
line_width = 90
stripe_color = (76, 129, 223)
margin_x = 80
usable_w = w - 2 * margin_x
gap = (usable_w - 4 * line_width) // 3
y_top = int(h * 0.175)
y_bottom = int(h * 0.825)
print(f'4 stripes: width={line_width}, gap={gap}, margin_x={margin_x}, y={y_top}-{y_bottom}')

for i in range(4):
    x_start = margin_x + i * (line_width + gap)
    x_end = x_start + line_width
    draw.rectangle([x_start, y_top, x_end - 1, y_bottom], fill=stripe_color)
    print(f'  stripe {i+1}: x={x_start}-{x_end}')

# 5. 把鲸鱼贴回去
new_pixels = new_img.load()
for y in range(h):
    for x in range(w):
        if wm_data[x, y] == 255:
            new_pixels[x, y] = pixels[x, y]

# 6. 应用圆角蒙版
result = Image.new('RGB', (w, h), (255, 255, 255))
result.paste(new_img, (0, 0), corner_mask)

result.save('app_icon_final_v2.png')
print('saved app_icon_final_v2.png')
