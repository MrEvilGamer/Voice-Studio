from pathlib import Path
from importlib import metadata
import json
import shutil
import sys

root = Path(__file__).resolve().parents[1] / 'src-tauri/tracker-runtime/eye-worker/licenses'
root.mkdir(exist_ok=True)
packages = ['mediapipe','opencv-python','numpy','customtkinter','Pillow','matplotlib',
            'protobuf','absl-py','attrs','flatbuffers','opt-einsum','sounddevice','cffi',
            'pycparser','packaging','six','python-dateutil','kiwisolver','contourpy',
            'cycler','fonttools','pyparsing','comtypes','pygrabber','darkdetect','psutil',
            'requests','urllib3','charset-normalizer','certifi','idna']
versions = {}
for name in packages:
    try:
        distribution = metadata.distribution(name)
    except metadata.PackageNotFoundError:
        continue
    versions[name] = distribution.version
    for file in distribution.files or []:
        if '.dist-info' in str(file) and any(word in str(file).lower() for word in ['license','copying','copyright','notice']):
            source = Path(distribution.locate_file(file))
            destination = root / name / Path(*file.parts[1:])
            destination.parent.mkdir(parents=True, exist_ok=True)
            if source.is_file(): shutil.copy2(source, destination)
license_file = Path(sys.base_prefix) / 'LICENSE.txt'
if license_file.exists(): shutil.copy2(license_file, root / 'Python-LICENSE.txt')
(root / 'versions.json').write_text(json.dumps(versions, indent=2), encoding='utf-8')
print(f'Bundled dependency notices for {len(versions)} packages.')
