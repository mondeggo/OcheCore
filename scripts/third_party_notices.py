#!/usr/bin/env python3
"""Bundle license texts from the locked dependency graph for one target."""
import argparse
import json
import os
from html.parser import HTMLParser
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output([
        os.environ.get("CARGO", "cargo"), "metadata", "--format-version", "1",
        "--locked", "--offline", "--filter-platform", args.target,
    ]))
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    pending = [p["id"] for p in packages.values() if p["name"] == "oche-cli" and p["source"] is None]
    visited = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        for dep in nodes[package_id]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])

    sections = ["OcheCore third-party notices", f"Target: {args.target}",
                "These components retain their respective license terms."]
    sysroot = Path(subprocess.check_output([
        os.environ.get("RUSTC", "rustc"), "--print", "sysroot"
    ], text=True).strip())
    rust_notices = sysroot / "share/doc/rust/COPYRIGHT-library.html"
    class Text(HTMLParser):
        def handle_data(self, data):
            sections.append(data)
    sections += ["=" * 72, "Rust Standard Library copyright and license notices"]
    Text().feed(rust_notices.read_text())
    if "musl" in args.target:
        sections += ["=" * 72, "musl C runtime", Path("licenses/musl-COPYRIGHT").read_text()]
    for package_id in sorted(visited, key=lambda p: (packages[p]["name"], packages[p]["version"])):
        package = packages[package_id]
        if package["source"] is None:
            continue
        root = Path(package["manifest_path"]).parent
        files = {p for p in root.rglob("*") if p.is_file() and p.name.upper().startswith(
            ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"))}
        if package.get("license_file"):
            files.add(root / package["license_file"])
        # These tarballs omit their repository's MIT text. Pin fallbacks to
        # audited releases and retain upstream provenance in licenses/README.md.
        fallback = {
            ("linux-video-core", "0.1.1"): "linux-video-core-0.1.1-LICENSE",
            ("parse-display", "0.8.2"): "parse-display-0.8.2-LICENSE-MIT",
            ("parse-display-derive", "0.8.2"): "parse-display-0.8.2-LICENSE-MIT",
            ("structmeta", "0.2.0"): "structmeta-0.2.0-LICENSE-MIT",
            ("structmeta-derive", "0.2.0"): "structmeta-0.2.0-LICENSE-MIT",
        }.get((package["name"], package["version"]))
        if not files and fallback:
            root = Path("licenses")
            files.add(root / fallback)
        if not files:
            raise SystemExit(f"Missing license text for {package['name']} {package['version']}; resolve before distribution")
        sections += ["=" * 72, f"{package['name']} {package['version']}",
                     f"Declared license: {package.get('license')}",
                     f"Source: {package['source']}"]
        for path in sorted(files):
            sections += [f"--- {path.relative_to(root)} ---", path.read_text()]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n\n".join(sections) + "\n")


if __name__ == "__main__":
    main()
