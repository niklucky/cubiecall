import importlib.util
import base64
import json
from pathlib import Path
import plistlib
import struct
import subprocess
import tempfile
import unittest
import zipfile


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


metadata = load("metadata", "ci-metadata.py")
release = load("release", "verify-release.py")


class ReleaseMetadataTest(unittest.TestCase):
    cargo = '[workspace.package]\nversion = "0.1.0"\n[dependencies]\n'

    def test_release_uses_tag_version_and_shared_build(self):
        result = metadata.resolve("push", "refs/tags/v2.13.7", 1, 1, self.cargo)
        self.assertEqual(result, {"version": "2.13.7", "build": "1101", "publish": "true"})

    def test_invalid_tags_cannot_publish(self):
        for tag in ("v1.2", "v01.2.3", "v1.2.3-beta.1", "v1.2.3+build", "v1.2.3/extra", "v1.2.3\n"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                metadata.resolve("push", "refs/tags/" + tag, 1, 1, self.cargo)

    def test_manual_run_never_publishes_even_on_a_tag(self):
        result = metadata.resolve("workflow_dispatch", "refs/tags/v9.0.0", 4, 1, self.cargo)
        self.assertEqual(result["publish"], "false")
        self.assertEqual(result["version"], "0.1.0")

    def test_builds_increase_across_attempts_and_runs(self):
        builds = [int(metadata.resolve("push", "refs/tags/v0.1.1", run, attempt, self.cargo)["build"])
                  for run, attempt in ((1, 1), (1, 2), (2, 1))]
        self.assertEqual(builds, sorted(set(builds)))
        self.assertGreater(builds[0], 108)

    def test_other_events_are_rejected(self):
        with self.assertRaises(ValueError):
            metadata.resolve("pull_request", "refs/pull/1/merge", 1, 1, self.cargo)


class SignedReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.original_public = release.PUBLIC_KEY
        self.addCleanup(setattr, release, "PUBLIC_KEY", self.original_public)
        self.private = self.root / "private.pem"
        subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(self.private)],
                       check=True, capture_output=True)
        public = subprocess.run(["openssl", "pkey", "-in", str(self.private), "-pubout", "-outform", "DER"],
                                check=True, capture_output=True).stdout
        release.PUBLIC_KEY = base64.b64encode(public[-32:]).decode()
        (self.root / "version.txt").write_text("1.2.3 (1101)\n")
        self.archive = self.root / "Cubie-apple-silicon.zip"
        with zipfile.ZipFile(self.archive, "w") as bundle:
            info = {"CFBundleIdentifier": "com.quipcall.desktop", "CFBundleShortVersionString": "1.2.3",
                    "CFBundleVersion": "1101", "SUPublicEDKey": release.PUBLIC_KEY}
            bundle.writestr("Cubie.app/Contents/Info.plist", plistlib.dumps(info))
            bundle.writestr("Cubie.app/Contents/MacOS/Cubie", struct.pack("<II", 0xFEEDFACF, 0x100000C))
        (self.root / "Cubie-apple-silicon-notary-log.json").write_text(json.dumps({"status": "Accepted"}))
        contents = self.archive.read_bytes()
        signature = self.sign(contents)
        feed = (f'<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>'
                f'<sparkle:version>1101</sparkle:version><sparkle:shortVersionString>1.2.3</sparkle:shortVersionString>'
                f'<enclosure url="https://cubiecall.com/download/releases/1101/{self.archive.name}" '
                f'length="{len(contents)}" sparkle:edSignature="{signature}"/></item></channel></rss>').encode()
        self.feed = self.root / "appcast-apple-silicon.xml"
        self.feed.write_bytes(feed + f'<!-- sparkle-signatures:\nedSignature: {self.sign(feed)}\nlength: {len(feed)}\n-->'.encode())

    def sign(self, message):
        source = self.root / "message"
        source.write_bytes(message)
        result = subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(self.private), "-rawin", "-in", str(source)],
                                capture_output=True, check=True)
        return base64.b64encode(result.stdout).decode()

    def test_valid_signed_release(self):
        self.assertEqual(len(release.verify(self.root, ["apple-silicon"], "1.2.3", "1101")), 3)

    def test_modified_feed_is_rejected(self):
        self.feed.write_bytes(self.feed.read_bytes().replace(b"1.2.3", b"1.2.4"))
        with self.assertRaisesRegex(ValueError, "signature verification failed"):
            release.verify(self.root, ["apple-silicon"])

    def test_modified_archive_is_rejected(self):
        with zipfile.ZipFile(self.archive, "a") as bundle:
            bundle.writestr("Cubie.app/injected-file", "unexpected")
        with self.assertRaisesRegex(ValueError, "archive length"):
            release.verify(self.root, ["apple-silicon"])

    def test_build_mismatch_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "Unexpected build"):
            release.verify(self.root, ["apple-silicon"], expected_build="1102")

    def test_unaccepted_notarization_is_rejected(self):
        (self.root / "Cubie-apple-silicon-notary-log.json").write_text('{"status":"Invalid"}')
        with self.assertRaisesRegex(ValueError, "Apple did not accept"):
            release.verify(self.root, ["apple-silicon"])


if __name__ == "__main__":
    unittest.main()
