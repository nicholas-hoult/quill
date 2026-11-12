"""生成应用图标（深色圆角底 + 琥珀色羽毛笔尖），输出 1024 PNG。用法：make_icon.py 输出.png"""
import sys
from PIL import Image, ImageDraw, ImageFilter

S = 1024
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
# 底：苹果图标网格内的圆角方块（留 10% 边距）
m, r = 100, 200
bg = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(bg)
d.rounded_rectangle((m, m, S - m, S - m), r, fill=(20, 20, 23, 255))
# 轻微的顶部高光
glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
ImageDraw.Draw(glow).ellipse((200, 40, 900, 560), fill=(255, 155, 94, 38))
glow = glow.filter(ImageFilter.GaussianBlur(90))
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle((m, m, S - m, S - m), r, fill=255)
bg = Image.alpha_composite(bg, Image.composite(glow, Image.new("RGBA", (S, S), (0, 0, 0, 0)), mask))
img = Image.alpha_composite(img, bg)
d = ImageDraw.Draw(img)
# 笔尖：一片斜放的叶形 + 笔杆 + 高光点
amber = (255, 155, 94, 255)
d.polygon([(330, 720), (690, 330), (760, 400), (400, 790)], fill=amber)
d.polygon([(300, 790), (330, 720), (400, 790)], fill=(255, 255, 255, 255))
d.ellipse((650, 250, 700, 300), fill=(255, 214, 180, 255))
img.save(sys.argv[1])
