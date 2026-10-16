#!/usr/bin/env python3
"""Fetch the pinned official Helm tool used by chart validation and disposable CI."""
import argparse
import hashlib
import io
from pathlib import Path
import platform
import tarfile
import urllib.request

VERSION='4.3.1'
PINS={('Linux','x86_64'):'0403a88ae20913d11b10a2467b50087721fea7581736da6b9d3992f5357fe54b',('Linux','aarch64'):'8cf8f66ecbc252ac8f7f06cca68aba88508ddfde3a6a34feb1678ad1694b4f88',('Darwin','arm64'):'6b4e61c8cd27d99e5a801e63ff25dd5c70b36875c2cd659d3e9ce6b76fc23198'}


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
    key=(platform.system(),platform.machine());checksum=PINS[key]
    osname={'Linux':'linux','Darwin':'darwin'}[key[0]];arch={'x86_64':'amd64','aarch64':'arm64','arm64':'arm64'}[key[1]]
    with urllib.request.urlopen(f'https://get.helm.sh/helm-v{VERSION}-{osname}-{arch}.tar.gz',timeout=60) as response:data=response.read(50*1024*1024+1)
    assert len(data)<=50*1024*1024 and hashlib.sha256(data).hexdigest()==checksum,'Helm archive checksum mismatch'
    with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as tar:
        member=tar.getmember(f'{osname}-{arch}/helm');assert member.isfile() and member.size<100*1024*1024
        binary=tar.extractfile(member).read()
    a.out.parent.mkdir(parents=True,exist_ok=True);a.out.write_bytes(binary);a.out.chmod(0o755)
    print('Verified Helm '+VERSION)


if __name__=='__main__':main()
