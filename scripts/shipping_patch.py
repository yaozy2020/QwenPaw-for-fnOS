"""Locate the exact Console payload shipped by this FPK."""
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def shipping_patch(root=ROOT):
    version = (root / 'patches/verified-version').read_text(encoding='ascii').strip()
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:\.post[0-9]+)?', version):
        raise ValueError('invalid patches/verified-version')
    target = root / 'patches' / f'qwenpaw-{version}-base-path'
    manifest = json.loads((target / 'manifest.json').read_bytes())
    if manifest.get('base_version') != version or manifest.get('console_complete') is not True:
        raise ValueError('shipping payload manifest does not match verified-version')
    return version, target


if __name__ == '__main__':
    print(shipping_patch()[1].name)
