#!/usr/bin/env python3
"""Builds a release's manifest.json and SHA256SUMS from the per-file
metadata the platform jobs write (scripts/release/r2-upload.sh).

    manifest.py <meta-dir> <version> <out-dir>

Reads every <meta-dir>/**/*.json ({file, size, sha256, platform, arch,
kind}) and writes <out-dir>/manifest.json and <out-dir>/SHA256SUMS. The
manifest is the contract buwiz.com reads:

    {"version": "1.2.0", "published_at": "<RFC 3339 UTC>",
     "files": [{"platform", "arch", "kind", "file", "size", "sha256"}]}

`file` is relative to releases/v<version>/. PUBLISHED_AT in the
environment overrides the timestamp (tests only).
"""

import json
import os
import pathlib
import re
import sys
from datetime import datetime, timezone

PLATFORMS = {"macos", "windows", "linux", "android"}
ARCHES = {"arm64", "x64", "universal"}
KINDS = {"dmg", "msi", "exe", "appimage", "deb", "rpm", "apk"}
KEYS = {"file", "size", "sha256", "platform", "arch", "kind"}


def fail(message):
    sys.exit(f"manifest: {message}")


def main(argv):
    if len(argv) != 4:
        fail("usage: manifest.py <meta-dir> <version> <out-dir>")
    meta_dir, version, out_dir = pathlib.Path(argv[1]), argv[2], pathlib.Path(argv[3])
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        fail(f"version must be MAJOR.MINOR.PATCH (got {version!r})")

    files = []
    for path in sorted(meta_dir.rglob("*.json")):
        entry = json.loads(path.read_text(encoding="utf-8"))
        if set(entry) != KEYS:
            fail(f"{path}: keys must be exactly {sorted(KEYS)}, got {sorted(entry)}")
        name = entry["file"]
        if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9._+-]+", name) or version not in name:
            fail(f"{path}: bad file name {name!r} (plain name carrying {version})")
        if not isinstance(entry["size"], int) or isinstance(entry["size"], bool) or entry["size"] <= 0:
            fail(f"{path}: size must be a positive integer")
        if not isinstance(entry["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]):
            fail(f"{path}: sha256 must be 64 lowercase hex digits")
        if entry["platform"] not in PLATFORMS:
            fail(f"{path}: unknown platform {entry['platform']!r}")
        if entry["arch"] not in ARCHES:
            fail(f"{path}: unknown arch {entry['arch']!r}")
        if entry["kind"] not in KINDS:
            fail(f"{path}: unknown kind {entry['kind']!r}")
        files.append({k: entry[k] for k in ("platform", "arch", "kind", "file", "size", "sha256")})

    if not files:
        fail(f"no metadata under {meta_dir}")
    names = [f["file"] for f in files]
    if len(set(names)) != len(names):
        fail(f"duplicate file names: {sorted(names)}")
    files.sort(key=lambda f: (f["platform"], f["arch"], f["kind"], f["file"]))

    published_at = os.environ.get("PUBLISHED_AT") or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    manifest = {"version": version, "published_at": published_at, "files": files}

    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sums = "".join(f"{f['sha256']}  {f['file']}\n" for f in sorted(files, key=lambda f: f["file"]))
    with open(out_dir / "SHA256SUMS", "w", encoding="utf-8", newline="\n") as out:
        out.write(sums)
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main(sys.argv)
