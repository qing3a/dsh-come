from PIL import Image, ImageDraw, ImageFilter
import math

# 1. 打开 E1 原图
img = Image.open('app_icon_e1.png').convert('RGB')
w, h = img.size
pixels = img.load()

bg_actual = (182, 212, 226)

# 2. 圆角蒙版
corner_mask = Image.new('L', (w, h), 0)
cm = corner_mask.load()
for y in range(h):
    for x in range(w):
        r, g, b = pixels[x, y]
        if r > 240 and g > 240 and b > 240:
            cm[x, y] = 0
        else:
            cm[x, y] = 255

# 3. 鲸鱼蒙版（颜色距离）
whale_mask = Image.new('L', (w, h), 0)
wm = whale_mask.load()
dist_threshold = 12
for y in range(h):
    for x in range(w):
        if cm[x, y] == 0:
            continue
        r, g, b = pixels[x, y]
        dist = math.sqrt((r - bg_actual[0])**2 + (g - bg_actual[1])**2 + (b - bg_actual[2])**2)
        if dist > dist_threshold:
            wm[x, y] = 255

# 中值滤波去噪点
whale_mask = whale_mask.filter(ImageFilter.MedianFilter(size=3))

# 统计鲸鱼像素
wm_data = whale_mask.load()
whale_count = sum(1 for y in range(h) for x in range(w) if wm_data[x, y] == 255)
print(f'whale mask pixels: {whale_count}, threshold: {dist_threshold}')

# 4. 新背景 + 4条粗竖线
new_bg = (200, 222, 235)
new_img = Image.new('RGB', (w, h), (255, 255, 255))
draw = ImageDraw.Draw(new_img)
draw.rectangle([0, 0, w - 1, h - 1], fill=new_bg)

line_width = 130
stripe_color = (70, 140, 225)
gap = (w - 4 * line_width) // 3
print(f'4 stripes: line_width={line_width}, gap={gap}')
for i in range(4):
    x_start = i * (line_width + gap)
    x_end = x_start + line_width
    print(f'  stripe {i+1}: x={x_start}-{x_end}')
    draw.rectangle([x_start, 0, x_end - 1, h - 1], fill=stripe_color)

# 5. 复制鲸鱼区域像素
new_pixels = new_img.load()
for y in range(h):
    for x in range(w):
        if wm_data[x, y] == 255:
            new_pixels[x, y] = pixels[x, y]

# 6. 圆角
result = Image.new('RGB', (w, h), (255, 255, 255))
result.paste(new_img, (0, 0), corner_mask)

result.save('app_icon_final.png')
print('saved app_icon_final.png')
