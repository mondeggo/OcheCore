#!/usr/bin/env python3
"""Package an already compiled binary, documentation, and license notices."""
import argparse
import gzip
import hashlib
import os
from pathlib import Path
import re
import tarfile
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=["amd64", "arm64"], required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--notices", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", version):
        raise SystemExit("Unsupported release version")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    name = f"ochecore-v{version}-linux-{args.arch}"
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / f"{name}.tar.gz"
    files = {"oche-core": args.binary, "THIRD_PARTY_NOTICES.txt": args.notices,
             "LICENSE": Path("LICENSE"), "NOTICE": Path("NOTICE"),
             "README.md": Path("README.md"), "AGENTS.md": Path("AGENTS.md"),
             "oche-core-roadmap.md": Path("oche-core-roadmap.md"), "compose.yaml": Path("compose.yaml")}
    files.update({str(path): path for path in sorted(Path("docs").glob("*.md"))})
    with archive.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode="w") as tar:
            for relative, source in files.items():
                info = tar.gettarinfo(str(source), arcname=f"{name}/{relative}")
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = epoch
                info.mode = 0o755 if relative == "oche-core" else 0o644
                with source.open("rb") as content:
                    tar.addfile(info, content)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (args.output / f"{archive.name}.sha256").write_text(f"{digest}  {archive.name}\n")
    print(archive)


if __name__ == "__main__":
    main()
