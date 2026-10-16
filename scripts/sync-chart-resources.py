#!/usr/bin/env python3
"""Keep Helm's embedded resources identical to the native installer templates."""
import argparse
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'crates/control/src/pull_install/templates'
TARGET = ROOT / 'charts/kimage/files/resources'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    source = (ROOT / 'crates/control/src/builder_install.rs').read_text()
    block = re.search(r'WORKER_CAPABILITIES: &\[&str\] = &\[(.*?)\];', source, re.S)
    assert block, 'missing canonical worker capabilities'
    capabilities = re.findall(r'"([A-Z_]+)"', block[1])
    assert capabilities and len(capabilities) == len(set(capabilities))
    files = {path.name: path.read_bytes() for path in SOURCE.iterdir() if path.suffix in ('.json', '.toml')}
    files['worker-capabilities.json'] = (json.dumps(capabilities, indent=2) + '\n').encode()
    if args.check:
        assert TARGET.is_dir() and {p.name for p in TARGET.iterdir()} == set(files), 'run scripts/sync-chart-resources.py'
        for name, data in files.items():
            assert (TARGET/name).read_bytes() == data, f'{name}: Helm/native resource drift'
        print('Helm and native installer resources match.')
        return
    TARGET.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        (TARGET/name).write_bytes(data)
    print('Synced canonical resources into the Helm chart.')


if __name__ == '__main__':
    main()
