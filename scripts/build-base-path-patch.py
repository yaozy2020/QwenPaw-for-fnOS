#!/usr/bin/env python3
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import tarfile
import zipfile

BACKEND_FILES = (
    "app/_app.py",
    "app/auth.py",
    "app/base_path.py",
    "app/channels/ingress.py",
    "app/routers/agent_scoped.py",
    "app/routers/providers.py",
    "app/routers/voice.py",
)
SOURCE_COMMIT = "0a21644e90d8ba37e20035b3bfb63880872cc2a2"
SOURCE_PATCH_SHA256 = "b0ba1b6e48df72f6c3f7188dbee23a33ce282ef605f2f46401c64356f92d8c2f"
PREVIOUS_SHA256 = {
    "app/_app.py": ["38541ce134eb38f7ec64a82125365400cc2f3ca69ea5aef86b6f1ba42500877c"],
    "app/base_path.py": ["d4a4091d8a9832b6cb09ab3985c69b0927c5817570dbf31ba79638e7513687f1"],
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def previous_patch_hashes(fpks):
    previous = {}
    for fpk in fpks:
        with tarfile.open(fpk, "r:gz") as outer:
            app_data = outer.extractfile("app.tgz").read()
        with tarfile.open(fileobj=io.BytesIO(app_data), mode="r:gz") as archive:
            member = next(
                item for item in archive.getmembers()
                if item.name.endswith("/patches/qwenpaw-2.3.0-base-path/manifest.json")
            )
            manifest = json.load(archive.extractfile(member))
        for entry in manifest["files"]:
            values = previous.setdefault(entry["path"], [])
            for value in [entry["sha256"], *entry.get("previous_sha256", [])]:
                if value not in values:
                    values.append(value)
    return previous


def add_previous_hashes(entry, relative, previous):
    values = set(PREVIOUS_SHA256.get(relative, []))
    values.update(previous.get(relative, []))
    if values:
        entry["previous_sha256"] = sorted(values)


def main():
    parser = argparse.ArgumentParser(
        description="Build the QwenPaw 2.3.0 + PR #8719 compatibility payload."
    )
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--official-wheel", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--console", type=Path, required=True)
    parser.add_argument("--previous-patch", type=Path, action="append", default=[])
    args = parser.parse_args()
    source = args.source.resolve()
    wheel = args.official_wheel.resolve()
    output = args.output.resolve()
    previous = previous_patch_hashes(args.previous_patch)
    if output.name != "qwenpaw-2.3.0-base-path":
        raise SystemExit("output directory must be named qwenpaw-2.3.0-base-path")

    files = []
    payload = {}
    with zipfile.ZipFile(wheel) as archive:
        official = {
            name: archive.read(name)
            for name in archive.namelist()
            if not name.endswith("/")
        }
        for relative in BACKEND_FILES:
            patched_path = source / "src/qwenpaw" / relative
            patched = patched_path.read_bytes()
            original_name = f"qwenpaw/{relative}"
            original = official.get(original_name)
            if original == patched:
                continue
            payload[relative] = patched
            entry = {
                "path": relative,
                "original_sha256": sha256(original) if original is not None else None,
                "sha256": sha256(patched),
            }
            add_previous_hashes(entry, relative, previous)
            files.append(entry)

        if args.console:
            console = args.console.resolve()
            if not (console / "index.html").is_file():
                raise SystemExit("console payload is missing index.html")
            for patched_path in sorted(console.rglob("*")):
                if not patched_path.is_file():
                    continue
                relative = f"console/{patched_path.relative_to(console).as_posix()}"
                patched = patched_path.read_bytes()
                original = official.get(f"qwenpaw/{relative}")
                if original == patched:
                    continue
                payload[relative] = patched
                entry = {
                    "path": relative,
                    "original_sha256": sha256(original) if original is not None else None,
                    "sha256": sha256(patched),
                }
                add_previous_hashes(entry, relative, previous)
                files.append(entry)

    if not files:
        raise SystemExit("no patch differences found")
    if output.exists():
        shutil.rmtree(output)
    for relative, data in payload.items():
        target = output / "payload" / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        target.chmod(0o644)
    manifest = {
        "schema_version": 1,
        "base_version": "2.3.0",
        "upstream_pr": 8719,
        "upstream_commit": SOURCE_COMMIT,
        "source_patch_sha256": SOURCE_PATCH_SHA256,
        "official_wheel_sha256": sha256(wheel.read_bytes()),
        "files": files,
    }
    (output / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"payload={output}")
    print(f"files={len(files)}")
    print(f"bytes={sum(len(data) for data in payload.values())}")


if __name__ == "__main__":
    main()
