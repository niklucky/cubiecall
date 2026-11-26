#!/usr/bin/env python3
"""Verify ZIP metadata and Sparkle Ed25519 signatures on macOS or Linux."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import re
import struct
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify_signature(message, signature, public_key):
    signature = base64.b64decode(signature, validate=True)
    require(len(signature) == 64, "Invalid Ed25519 signature length")
    public = base64.b64decode(public_key, validate=True)
    require(len(public) == 32, "Invalid Sparkle public key")
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "public.der").write_bytes(bytes.fromhex("302a300506032b6570032100") + public)
        (root / "message").write_bytes(message)
        (root / "signature").write_bytes(signature)
        result = subprocess.run([
            "openssl", "pkeyutl", "-verify", "-pubin", "-keyform", "DER",
            "-inkey", str(root / "public.der"), "-rawin", "-in", str(root / "message"),
            "-sigfile", str(root / "signature"),
        ], capture_output=True)
        require(result.returncode == 0, "Sparkle signature verification failed")


def verify(root, architectures, expected_version=None, expected_build=None, *, public_url, public_key):
    public_url = public_url.rstrip("/")
    metadata = re.fullmatch(r"([0-9]+\.[0-9]+\.[0-9]+) \(([1-9][0-9]*)\)\n?", (root / "version.txt").read_text())
    require(metadata is not None, "Invalid version.txt")
    version, build = metadata.groups()
    require(expected_version in (None, version), "Unexpected app version")
    require(expected_build in (None, build), "Unexpected build number")
    files = ["version.txt"]
    for arch in architectures:
        archive = root / f"Cubie-{arch}.zip"
        feed_path = root / f"appcast-{arch}.xml"
        report = json.loads((root / f"Cubie-{arch}-notary-log.json").read_text())
        require(report["status"] == "Accepted", f"{arch}: Apple did not accept this build")
        with zipfile.ZipFile(archive) as bundle:
            names = bundle.namelist()
            require(len(names) == len(set(names)), "Duplicate ZIP entries")
            require(all(not PurePosixPath(n).is_absolute() and ".." not in PurePosixPath(n).parts for n in names), "Unsafe ZIP path")
            info = plistlib.loads(bundle.read("Cubie.app/Contents/Info.plist"))
            require(info["CFBundleIdentifier"] == "com.quipcall.desktop", "Wrong bundle identifier")
            require(info["CFBundleShortVersionString"] == version, "ZIP version mismatch")
            require(info["CFBundleVersion"] == build, "ZIP build mismatch")
            require(info["SUPublicEDKey"] == public_key, "Sparkle public key changed")
            require(info["SUFeedURL"] == f"{public_url}/download/current/appcast-{arch}.xml", "Wrong feed URL")
            with bundle.open("Cubie.app/Contents/MacOS/Cubie") as binary:
                magic, cpu = struct.unpack("<II", binary.read(8))
            require(magic == 0xFEEDFACF and cpu == {"apple-silicon": 0x100000C, "intel": 0x1000007}[arch], "ZIP architecture mismatch")
        feed = feed_path.read_bytes()
        signature = re.search(rb"<!-- sparkle-signatures:\s*edSignature: ([A-Za-z0-9+/=]+)\s*length: ([0-9]+)\s*-->\s*$", feed)
        require(signature is not None, "Appcast is not signed")
        length = int(signature[2])
        require(length == signature.start(), "Appcast signed length mismatch")
        verify_signature(feed[:length], signature[1], public_key)
        items = ET.fromstring(feed).findall("./channel/item")
        require(len(items) == 1, "Expected exactly one release in each architecture feed")
        item = items[0]
        require(item.findtext(SPARKLE + "version") == build, "Appcast build mismatch")
        require(item.findtext(SPARKLE + "shortVersionString") == version, "Appcast version mismatch")
        enclosure = item.find("enclosure")
        require(enclosure is not None, "Missing enclosure")
        require(enclosure.attrib["url"] == f"{public_url}/download/releases/{build}/{archive.name}", "Wrong download URL")
        contents = archive.read_bytes()
        require(int(enclosure.attrib["length"]) == len(contents), "Wrong archive length")
        verify_signature(contents, enclosure.attrib[SPARKLE + "edSignature"], public_key)
        files.extend([archive.name, feed_path.name])
        print(f"{arch}: accepted notarization, correct version/build/architecture, valid appcast and ZIP signatures")
    return files


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--arch", choices=["apple-silicon", "intel"])
    parser.add_argument("--version")
    parser.add_argument("--build")
    parser.add_argument("--write-checksums", action="store_true")
    args = parser.parse_args()
    try:
        files = verify(
            args.directory, [args.arch] if args.arch else ["apple-silicon", "intel"], args.version, args.build,
            public_url=os.environ["CUBIE_PUBLIC_URL"], public_key=os.environ["CUBIE_SPARKLE_PUBLIC_KEY"],
        )
        if args.write_checksums:
            (args.directory / "SHA256SUMS").write_text("".join(
                f"{hashlib.sha256((args.directory / name).read_bytes()).hexdigest()}  {name}\n" for name in files))
    except (ValueError, KeyError, OSError, ET.ParseError, zipfile.BadZipFile) as error:
        raise SystemExit(f"Release verification failed: {error}") from error
