"""Version checks, release archives and retry-safe registry inspection."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tarfile
import tomllib
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import zipfile

ROOT = Path(__file__).resolve().parents[2]
API_MANIFEST = Path("crates/source-api/Cargo.toml")
CLI_MANIFEST = Path("bin/downloader-cli/Cargo.toml")
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)


def package(manifest, root=ROOT):
    return tomllib.loads((root / manifest).read_text())["package"]


def output(**values):
    for key, value in values.items():
        line = f"{key}={value}"
        print(line)
        if path := os.environ.get("GITHUB_OUTPUT"):
            with open(path, "a", encoding="utf-8") as stream:
                stream.write(line + "\n")


def validate(tag="", root=ROOT):
    version = package(CLI_MANIFEST, root)["version"]
    match = SEMVER.fullmatch(version)
    if not match or any(
        part.isdigit() and len(part) > 1 and part.startswith("0")
        for part in (match[4] or "").split(".")
    ):
        raise ValueError(f"Invalid release version: {version}")
    if package(API_MANIFEST, root)["version"] != version:
        raise ValueError("CLI and source-api versions must match before a release")
    if tag and tag != f"v{version}":
        raise ValueError(f"Tag {tag!r} does not match manifest version v{version}")
    return version, "true" if match[4] else "false"


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def pack(target, binary, archive, root=ROOT):
    source = root / "target" / target / "release" / binary
    if archive != Path(archive).name or not archive.startswith("dc-"):
        raise ValueError("Archive must be a dc-* file name")
    dest = root / "release-assets" / archive
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.suffix == ".zip":
        with zipfile.ZipFile(dest, "w", zipfile.ZIP_DEFLATED) as stream:
            stream.write(source, arcname=binary)
    elif archive.endswith(".tar.gz"):
        with tarfile.open(dest, "w:gz") as stream:
            stream.add(source, arcname=binary)
    else:
        raise ValueError("Expected a .zip or .tar.gz archive")
    dest.with_name(dest.name + ".sha256").write_text(
        f"{sha256(dest)}  {dest.name}\n", encoding="utf-8"
    )
    return dest


def verify_assets(count, root=ROOT):
    folder = root / "release-assets"
    archives = sorted(p for p in folder.glob("dc-*") if not p.name.endswith(".sha256"))
    if len(archives) != count:
        raise ValueError(f"Expected {count} archives, found {len(archives)}")
    lines = []
    for archive in archives:
        expected = f"{sha256(archive)}  {archive.name}\n"
        if archive.with_name(archive.name + ".sha256").read_text() != expected:
            raise ValueError(f"Checksum mismatch: {archive.name}")
        lines.append(expected)
    (folder / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")


def registry_status(root=ROOT):
    metadata = package(API_MANIFEST, root)
    name, version = metadata["name"], metadata["version"]
    archive = root / "target" / "package" / f"{name}-{version}.crate"
    digest = sha256(archive)
    request = Request(
        f"https://crates.io/api/v1/crates/{name}/{version}",
        headers={"User-Agent": "downloader-release (github.com/badawsome/downloader)"},
    )
    try:
        with urlopen(request, timeout=30) as response:
            published = json.load(response)["version"]
    except HTTPError as error:
        if error.code == 404:
            return "false"
        raise
    if published["checksum"] != digest:
        raise ValueError(f"{name} {version} already exists with different contents; bump versions")
    return "true"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["validate", "pack", "verify-assets", "registry"])
    parser.add_argument("--count", type=int, default=9)
    args = parser.parse_args()
    if args.command == "validate":
        version, prerelease = validate(os.environ.get("RELEASE_TAG", ""))
        output(version=version, prerelease=prerelease)
    elif args.command == "pack":
        pack(os.environ["TARGET"], os.environ["BINARY"], os.environ["ARCHIVE"])
    elif args.command == "verify-assets":
        verify_assets(args.count)
    else:
        output(published=registry_status())


if __name__ == "__main__":
    main()
