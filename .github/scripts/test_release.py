"""Offline checks for release inputs, archives, completeness and retry handling."""

import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for manifest in [release.API_MANIFEST, release.CLI_MANIFEST]:
            path = self.root / manifest
            path.parent.mkdir(parents=True)
            path.write_text('[package]\nname = "source-api"\nversion = "0.2.0"\n')

    def test_tag_must_match_both_manifests(self):
        self.assertEqual(release.validate("v0.2.0", self.root), ("0.2.0", "false"))
        for tag in ["0.2.0", "v0.3.0", "v0.2.0-extra", "v../../bad"]:
            with self.assertRaises(ValueError):
                release.validate(tag, self.root)
        path = self.root / release.API_MANIFEST
        path.write_text(path.read_text().replace("0.2.0", "0.1.0"))
        with self.assertRaises(ValueError):
            release.validate("v0.2.0", self.root)

    def test_prerelease_and_invalid_versions(self):
        for version, prerelease in [("0.3.0-rc.1", "true"), ("1.0.0+build.1", "false")]:
            for manifest in [release.API_MANIFEST, release.CLI_MANIFEST]:
                (self.root / manifest).write_text(f'[package]\nversion = "{version}"\n')
            self.assertEqual(release.validate(f"v{version}", self.root), (version, prerelease))
        for version in ["01.2.0", "0.2.0-01", "0.2.0-", "0.2"]:
            (self.root / release.CLI_MANIFEST).write_text(f'[package]\nversion = "{version}"\n')
            with self.assertRaises(ValueError):
                release.validate(root=self.root)

    def test_both_archive_formats_and_checksums(self):
        for binary, archive in [("dc", "dc-Linux.tar.gz"), ("dc.exe", "dc-Windows.zip")]:
            source = self.root / "target/test/release" / binary
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_bytes(b"test executable")
            source.chmod(0o755)
            dest = release.pack("test", binary, archive, self.root)
            if archive.endswith(".zip"):
                with zipfile.ZipFile(dest) as stream:
                    self.assertEqual(stream.namelist(), [binary])
                    self.assertEqual(stream.read(binary), source.read_bytes())
            else:
                with tarfile.open(dest) as stream:
                    self.assertEqual(stream.getnames(), [binary])
                    self.assertTrue(stream.getmember(binary).mode & 0o111)
                    self.assertEqual(stream.extractfile(binary).read(), source.read_bytes())
        release.verify_assets(2, self.root)
        self.assertEqual(len((self.root / "release-assets/SHA256SUMS").read_text().splitlines()), 2)
        with self.assertRaises(ValueError):
            release.verify_assets(9, self.root)
        dest.write_bytes(b"corrupted")
        with self.assertRaises(ValueError):
            release.verify_assets(2, self.root)

    def test_registry_retries_require_identical_contents(self):
        archive = self.root / "target/package/source-api-0.2.0.crate"
        archive.parent.mkdir(parents=True)
        archive.write_bytes(b"package")
        same = {"version": {"checksum": release.sha256(archive)}}
        with patch("release.urlopen", return_value=io.BytesIO(json.dumps(same).encode())):
            self.assertEqual(release.registry_status(self.root), "true")
        different = {"version": {"checksum": "0" * 64}}
        with patch("release.urlopen", return_value=io.BytesIO(json.dumps(different).encode())):
            with self.assertRaises(ValueError):
                release.registry_status(self.root)
        with patch("release.urlopen", side_effect=HTTPError("url", 404, "missing", {}, None)):
            self.assertEqual(release.registry_status(self.root), "false")
        with patch("release.urlopen", side_effect=HTTPError("url", 503, "unavailable", {}, None)):
            with self.assertRaises(HTTPError):
                release.registry_status(self.root)


if __name__ == "__main__":
    unittest.main()
