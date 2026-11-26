#!/usr/bin/env python3
"""Zip a built cubie-desktop.exe the way testers download it.

    python deploy/windows/package.py <cubie-desktop.exe> [out dir, default target/windows]

Writes Cubie-windows-x64.zip (Cubie.exe and README.txt), version.txt
("0.1.0 (1234)", as the Mac's) and SHA256SUMS. CUBIE_VERSION and CUBIE_BUILD
name the version, as they do for the build itself.
"""
import hashlib
import os
from pathlib import Path
import re
import sys
import zipfile

ZIP = "Cubie-windows-x64.zip"

# CRLF: older Notepads show LF-only text on one line.
README = """\
Cubie for Windows (test build)

1. Unzip this folder: right-click it, Extract All. Cubie doesn't run from
   inside the zip.
2. Open Cubie.exe.
3. This build isn't signed yet, so Windows may say "Windows protected your
   PC". Click "More info", then "Run anyway". It asks once per new copy.

Cubie asks for the email you were invited with and sends it a code.
Windows 10 or 11, 64-bit.

Cubie doesn't update itself on Windows yet. For a newer version, download
it again from https://cubiecall.com/download/ and replace Cubie.exe.

Something broke? Send the log: %LOCALAPPDATA%\\Cubie\\Logs\\cubie.log
(paste the path into File Explorer's address bar).
""".replace("\n", "\r\n")


def workspace_version():
    cargo = (Path(__file__).resolve().parents[2] / "Cargo.toml").read_text()
    section = cargo.split("[workspace.package]", 1)[1].split("[", 1)[0]
    return re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)[1]


def main():
    if len(sys.argv) not in (2, 3):
        raise SystemExit(__doc__)
    exe = Path(sys.argv[1])
    out = Path(sys.argv[2] if len(sys.argv) == 3 else "target/windows")
    if not exe.is_file() or exe.read_bytes()[:2] != b"MZ":
        raise SystemExit(f"Not a Windows executable: {exe}")
    version = os.environ.get("CUBIE_VERSION") or workspace_version()
    build = os.environ.get("CUBIE_BUILD", "")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise SystemExit(f"Version must be X.Y.Z, not {version!r}")
    if not re.fullmatch(r"[0-9]+", build):
        raise SystemExit("Set CUBIE_BUILD to the build number")

    out.mkdir(parents=True, exist_ok=True)
    archive = out / ZIP
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        z.write(exe, "Cubie.exe")
        z.writestr("README.txt", README)
    # Bytes, not text: written on Windows, these would get CRLF, which the
    # publisher's build-number match and sha256sum on the host don't take.
    (out / "version.txt").write_bytes(f"{version} ({build})\n".encode())
    sums = "".join(
        f"{hashlib.sha256((out / name).read_bytes()).hexdigest()}  {name}\n"
        for name in (ZIP, "version.txt")
    )
    (out / "SHA256SUMS").write_bytes(sums.encode())
    print(f"{archive}: {archive.stat().st_size / 1e6:.1f} MB, {version} ({build})")


if __name__ == "__main__":
    main()
