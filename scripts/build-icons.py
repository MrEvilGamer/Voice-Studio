"""Package Voice Studio's approved emblem for Tauri and Windows. Requires Pillow."""
from pathlib import Path
from PIL import Image
ROOT = Path(__file__).resolve().parents[1]
source = Image.open(ROOT / 'public/brand/voice-studio-emblem.png').convert('RGBA')
assert source.getchannel('A').getextrema()[0] == 0, 'Logo must have transparent pixels'
bounds = source.getchannel('A').getbbox()
mark = source.crop(bounds)
canvas = Image.new('RGBA', (1024, 1024))
mark.thumbnail((920, 920), Image.Resampling.LANCZOS)
canvas.alpha_composite(mark, ((1024-mark.width)//2, (1024-mark.height)//2))
icons = ROOT / 'src-tauri/icons'
for size, name in [(32,'32x32.png'), (128,'128x128.png'), (512,'icon.png')]:
    canvas.resize((size,size),Image.Resampling.LANCZOS).save(icons/name)
canvas.save(icons/'icon.ico', sizes=[(s,s) for s in [16,24,32,48,64,128,256]])
canvas.resize((256,256),Image.Resampling.LANCZOS).save(ROOT/'public/brand/voice-studio-logo.png')
for name in ['32x32.png','128x128.png','icon.png']:
    with Image.open(icons/name) as image:
        assert image.mode=='RGBA' and image.width==image.height
print('Packaged transparent VS logo, PNG icons and seven-size Windows ICO.')
