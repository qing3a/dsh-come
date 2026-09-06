from PIL import Image

# 背景参数
W, H = 1024, 1024
bg_color = (232, 244, 252)  # #e8f4fc 极浅蓝
stripe_color = (125, 211, 252)  # #7dd3fc 天蓝
line_w = 130
gap = (W - 4 * line_w) // 3

def make_background():
    img = Image.new('RGB', (W, H), bg_color)
    from PIL import ImageDraw
    draw = ImageDraw.Draw(img)
    for i in range(4):
        x0 = i * (line_w + gap)
        x1 = x0 + line_w
        draw.rectangle([x0, 0, x1, H], fill=stripe_color)
    return img

def extract_whale(whale_path, output_path, whale_size=620, offset_x=0, offset_y=0):
    # 打开鲸鱼标志（白底）
    whale = Image.open(whale_path).convert('RGBA')
    # 抠图：白色背景变透明
    wdata = whale.load()
    for y in range(whale.height):
        for x in range(whale.width):
            r, g, b, a = wdata[x, y]
            if r > 240 and g > 240 and b > 240:
                wdata[x, y] = (r, g, b, 0)
    # 缩放鲸鱼
    whale.thumbnail((whale_size, whale_size), Image.LANCZOS)
    # 创建背景
    bg = make_background()
    # 贴鲸鱼（居中）
    px = (W - whale.width) // 2 + offset_x
    py = (H - whale.height) // 2 + offset_y
    bg.paste(whale, (px, py), whale)
    # 圆角
    mask = Image.new('L', (W, H), 0)
    from PIL import ImageDraw
    md = ImageDraw.Draw(mask)
    md.rounded_rectangle([0, 0, W-1, H-1], radius=120, fill=255)
    result = Image.new('RGB', (W, H), (255, 255, 255))
    result.paste(bg, (0, 0), mask)
    result.save(output_path)
    print(f'saved {output_path}')

# 下载两个鲸鱼标志
import subprocess
subprocess.run(['curl.exe', '-sL', '-o', 'whale_k1.png', 'https://aka.doubaocdn.com/s/yp4MRz8fuR'], check=True)
subprocess.run(['curl.exe', '-sL', '-o', 'whale_k2.png', 'https://aka.doubaocdn.com/s/zTm6tTkVEb'], check=True)

# 合成
extract_whale('whale_k1.png', 'app_icon_k1.png', whale_size=600)
extract_whale('whale_k2.png', 'app_icon_k2.png', whale_size=620)
