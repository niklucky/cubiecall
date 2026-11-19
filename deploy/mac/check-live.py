#!/usr/bin/env python3
"""Fetch public downloads and compare them with the verified release files."""
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(sys.argv[1])
files = [("version.txt", root / "version.txt"), ("", Path("deploy/download/index.html")),
         ("install.sh", Path("deploy/download/install.sh"))]
build = (root / "version.txt").read_text().split("(")[1].split(")")[0]
for arch in ("apple-silicon", "intel"):
    files.extend([(f"Cubie-{arch}.zip", root / f"Cubie-{arch}.zip"),
                  (f"releases/{build}/Cubie-{arch}.zip", root / f"Cubie-{arch}.zip"),
                  (f"current/appcast-{arch}.xml", root / f"appcast-{arch}.xml")])
with tempfile.TemporaryDirectory() as directory:
    download = Path(directory) / "download"
    for remote, local in files:
        url = "https://cubiecall.com/download/" + remote
        subprocess.run(["curl", "-fLsS", "--retry", "3", "--connect-timeout", "15",
                        "--max-time", "180", url, "-o", str(download)], check=True)
        if hashlib.sha256(download.read_bytes()).digest() != hashlib.sha256(local.read_bytes()).digest():
            raise SystemExit(f"Published content mismatch: {url}")
        print(f"Verified {url}", flush=True)
