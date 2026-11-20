"""Bundle the font families and weights used by Flare's desktop shell."""
from pathlib import Path
import urllib.request
import re
ROOT=Path(__file__).resolve().parents[1]
out=ROOT/'public/fonts';out.mkdir(parents=True,exist_ok=True)
styles=[]
for family,name,weights in [('Sora','sora',[400,500,600,700]),('Space+Grotesk','space-grotesk',[500,600,700]),('IBM+Plex+Mono','ibm-plex-mono',[500,600])]:
    for weight in weights:
        url=f'https://fonts.googleapis.com/css2?family={family}:wght@{weight}&display=swap'
        request=urllib.request.Request(url,headers={'User-Agent':'Mozilla/5.0 Chrome/120.0.0.0 Safari/537.36'})
        css=urllib.request.urlopen(request,timeout=25).read().decode()
        block=re.findall(r'@font-face\s*\{[^}]+\}',css)[-1]
        font_url=re.search(r'url\((https:[^)]+)\)',block).group(1)
        filename=f'{name}-{weight}.'+font_url.rsplit('.',1)[-1]
        (out/filename).write_bytes(urllib.request.urlopen(font_url,timeout=25).read())
        styles.append(block.replace(font_url,'/fonts/'+filename))
(ROOT/'src/fonts.css').write_text('\n'.join(styles),encoding='utf-8')
for family in ['sora','spacegrotesk','ibmplexmono']:
    url=f'https://raw.githubusercontent.com/google/fonts/main/ofl/{family}/OFL.txt'
    (out/(family+'-OFL.txt')).write_bytes(urllib.request.urlopen(url,timeout=25).read())
print('Bundled Sora, Space Grotesk and IBM Plex Mono, with OFL licenses.')
