from PIL import Image
import os

ico_path = r'C:\Users\Administrator\Desktop\dsh-come\extracted_icon.ico'
out_dir = r'C:\Users\Administrator\Desktop\dsh-come'

ico = Image.open(ico_path)
sizes = sorted(ico.info.get('sizes', [(256, 256)]), key=lambda s: s[0] * s[1], reverse=True)
print('Available sizes:', sizes)

largest = sizes[0]
print('Using largest:', largest)

ico.size = largest
img = ico.convert('RGBA')

# Original 256
p256 = os.path.join(out_dir, 'icon_256.png')
img.save(p256, 'PNG')
print('Saved', p256, img.size)

# 512
p512 = os.path.join(out_dir, 'icon_512.png')
img.resize((512, 512), Image.LANCZOS).save(p512, 'PNG')
print('Saved', p512)

# 1024
p1024 = os.path.join(out_dir, 'icon_1024.png')
img.resize((1024, 1024), Image.LANCZOS).save(p1024, 'PNG')
print('Saved', p1024)

print('Done!')
