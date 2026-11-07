#!/usr/bin/env python3
"""Builds a release's manifest.json and SHA256SUMS from the built files.

    manifest.py <files-dir> <version> <out-dir>

<files-dir> holds one folder per file, as the release workflow downloads
its artifacts: release-file-{platform}-{arch}-{kind}/<the one file>. The
manifest is the shape buwiz.com reads (src/sites/downloads.rs there), the
same as eBIRForms':

    {"version": "0.1.0", "published_at": "<RFC 3339 UTC>",
     "files": [{"platform", "arch", "kind", "file", "size", "sha256"}]}

`file` is relative to bot/releases/v<version>/. PUBLISHED_AT in the
environment overrides the timestamp (tests only).
"""

import hashlib
import json
import os
import pathlib
import re
import sys
from datetime import datetime, timezone

PLATFORMS = {"macos", "windows", "linux"}
ARCHES = {"arm64", "x64", "universal"}
KINDS = {"dmg", "msi", "exe", "appimage", "deb", "rpm", "targz"}


def fail(message):
    sys.exit(f"manifest: {message}")


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def entry(folder, version):
    parts = folder.name.removeprefix("release-file-").split("-")
    if len(parts) != 3:
        fail(f"{folder.name}: expected release-file-<platform>-<arch>-<kind>")
    platform, arch, kind = parts
    if platform not in PLATFORMS or arch not in ARCHES or kind not in KINDS:
        fail(f"{folder.name}: unknown platform, arch or kind")
    found = [path for path in folder.iterdir() if path.is_file()]
    if len(found) != 1:
        fail(f"{folder.name} holds {len(found)} files, expected one")
    path = found[0]
    carries_version = re.search(rf"(?<![0-9.]){re.escape(version)}(?![0-9]|\.[0-9])", path.name)
    if not re.fullmatch(r"[A-Za-z0-9._+-]+", path.name) or not carries_version:
        fail(f"{folder.name}: bad file name {path.name!r} (a plain name carrying {version})")
    size = path.stat().st_size
    if size == 0:
        fail(f"{path} is empty")
    return {"platform": platform, "arch": arch, "kind": kind, "file": path.name, "size": size, "sha256": sha256(path)}


def main(argv):
    if len(argv) != 4:
        fail("usage: manifest.py <files-dir> <version> <out-dir>")
    files_dir, version, out_dir = pathlib.Path(argv[1]), argv[2], pathlib.Path(argv[3])
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        fail(f"version must be MAJOR.MINOR.PATCH, not {version!r}")

    files = [entry(folder, version) for folder in sorted(files_dir.glob("release-file-*"))]
    if not files:
        fail(f"no release-file-* folders in {files_dir}")

    published_at = os.environ.get("PUBLISHED_AT") or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    manifest = {"version": version, "published_at": published_at, "files": files}
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sums = "".join(f"{f['sha256']}  {f['file']}\n" for f in sorted(files, key=lambda f: f["file"]))
    (out_dir / "SHA256SUMS").write_text(sums, encoding="utf-8")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main(sys.argv)
