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
    "app/console_static.py",
    "app/channels/ingress.py",
    "app/routers/agent_scoped.py",
    "app/routers/providers.py",
    "app/routers/voice.py",
    "browser/control_link/chrome/ws_handler.py",
    "cli/http.py",
)
EXTRA_CLIENTS = ("agents/tools/agent_management.py", "utils/startup_display.py")
POST1_FILES = ("app/fnos_entry.py", "cli/auto.py", "cli/cron_cmd.py", "cli/doctor_checks.py",
               "cli/plugin_commands.py", "cli/process_utils.py", *EXTRA_CLIENTS)
SOURCE_COMMITS = {
    "2.3.0": "0a21644e90d8ba37e20035b3bfb63880872cc2a2",
    "2.3.0.post1": "ee58cb1a64f2b21e825c525f73dc44bc7941edea",
}
SOURCE_PATCH_SHA256 = {
    "2.3.0": "c65666afb8f7c90e22d457b90f3d9a7e4be070e1b6e6c77d3c4b1710e46b3b20",
    "2.3.0.post1": "3cb051db72b16e106d9c1b8e2d420e451d892ad9475c73323abea01f24a082f4",
}
PREVIOUS_SHA256 = {
    "app/_app.py": ["38541ce134eb38f7ec64a82125365400cc2f3ca69ea5aef86b6f1ba42500877c"],
    "app/base_path.py": ["d4a4091d8a9832b6cb09ab3985c69b0927c5817570dbf31ba79638e7513687f1"],
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def safe_relative(value, console_only=False):
    if not isinstance(value, str):
        raise ValueError("patch path must be a string")
    parts = value.split("/")
    allowed_roots = ("console",) if console_only else ("app", "browser", "cli", "console")
    if (
        len(value) > 4096
        or len(parts) < 2
        or (parts[0] not in allowed_roots and (console_only or value not in EXTRA_CLIENTS))
        or any(part in ("", ".", "..") for part in parts)
        or any(char in value for char in ("\\", ":"))
        or any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in value)
    ):
        raise ValueError(f"unsafe patch path: {value!r}")
    return value


def valid_digest(value):
    if not isinstance(value, str) or len(value) != 64 or any(
        char not in "0123456789abcdefABCDEF" for char in value
    ):
        raise ValueError("patch manifest contains an invalid SHA256")
    return value.lower()


def merge_previous_manifest(previous, manifest, base_version):
    if manifest.get("schema_version") != 1 or manifest.get("base_version") != base_version:
        raise ValueError(f"previous patch manifest is not schema 1 / QwenPaw {base_version}")
    paths = set()
    for key in ("files", "delete_files"):
        for entry in manifest.get(key, []):
            relative = safe_relative(entry["path"], console_only=key == "delete_files")
            if relative in paths:
                raise ValueError(f"duplicate previous patch path: {relative}")
            paths.add(relative)
            values = previous.setdefault(relative, [])
            primary = entry["sha256"] if key == "files" else entry["original_sha256"]
            for value in [primary, *entry.get("previous_sha256", [])]:
                value = valid_digest(value)
                if value not in values:
                    values.append(value)


def previous_patch_hashes(fpks, manifests=(), base_version="2.3.0"):
    previous = {}
    patch_directory = f"qwenpaw-{base_version}-base-path"
    for fpk in fpks:
        with tarfile.open(fpk, "r:gz") as outer:
            app_data = outer.extractfile("app.tgz").read()
        with tarfile.open(fileobj=io.BytesIO(app_data), mode="r:gz") as archive:
            member = next(
                item for item in archive.getmembers()
                if item.name.endswith(f"/patches/{patch_directory}/manifest.json")
            )
            manifest = json.load(archive.extractfile(member))
        merge_previous_manifest(previous, manifest, base_version)
    for path in manifests:
        merge_previous_manifest(previous, json.loads(path.read_text(encoding="utf-8-sig")), base_version)
    return previous


def add_previous_hashes(entry, relative, previous):
    values = {valid_digest(value) for value in PREVIOUS_SHA256.get(relative, [])}
    values.update(previous.get(relative, []))
    if values:
        entry["previous_sha256"] = sorted(values)


def console_sources(console):
    if console.is_symlink() or not console.is_dir():
        raise ValueError("console payload must be a real directory")
    result = {}
    pending = [console]
    while pending:
        directory = pending.pop()
        for path in sorted(directory.iterdir()):
            if path.is_symlink():
                raise ValueError(f"console payload contains a symlink: {path}")
            if path.is_dir():
                pending.append(path)
            elif path.is_file():
                relative = safe_relative(
                    f"console/{path.relative_to(console).as_posix()}", console_only=True
                )
                result[relative] = path
            else:
                raise ValueError(f"console payload contains a special file: {path}")
    if "console/index.html" not in result:
        raise ValueError("console payload is missing index.html")
    return result


def main():
    parser = argparse.ArgumentParser(
        description="Build a version-matched QwenPaw + PR #8719 compatibility payload."
    )
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--official-wheel", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--console", type=Path, required=True)
    parser.add_argument("--version", choices=SOURCE_COMMITS, default="2.3.0")
    parser.add_argument("--previous-patch", type=Path, action="append", default=[])
    parser.add_argument("--previous-manifest", type=Path, action="append", default=[])
    args = parser.parse_args()
    source = args.source.resolve()
    wheel = args.official_wheel.resolve()
    output = args.output.resolve()
    previous = previous_patch_hashes(args.previous_patch, args.previous_manifest, args.version)
    console_paths = console_sources(args.console.absolute())
    expected_output = f"qwenpaw-{args.version}-base-path"
    if output.name != expected_output:
        raise SystemExit(f"output directory must be named {expected_output}")

    files = []
    delete_files = []
    payload = {}
    with zipfile.ZipFile(wheel) as archive:
        official = {
            name: archive.read(name)
            for name in archive.namelist()
            if not name.endswith("/")
        }
        for relative in (*BACKEND_FILES, *(POST1_FILES if args.version == "2.3.0.post1" else ())):
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

        for relative, patched_path in sorted(console_paths.items()):
            patched = patched_path.read_bytes()
            original = official.get(f"qwenpaw/{relative}")
            payload[relative] = patched
            entry = {
                "path": relative,
                "original_sha256": sha256(original) if original is not None else None,
                "sha256": sha256(patched),
            }
            add_previous_hashes(entry, relative, previous)
            files.append(entry)

        # Compare against the complete Console input, including unchanged wheel files.
        # Only this explicit list is removed at runtime.
        obsolete = {
            name.removeprefix("qwenpaw/")
            for name in official
            if name.startswith("qwenpaw/console/")
        }
        obsolete.update(relative for relative in previous if relative.startswith("console/"))
        for relative in sorted(obsolete - console_paths.keys()):
            safe_relative(relative, console_only=True)
            original = official.get(f"qwenpaw/{relative}")
            known = sorted(previous.get(relative, []))
            entry = {
                "path": relative,
                "original_sha256": sha256(original) if original is not None else known[0],
            }
            add_previous_hashes(entry, relative, previous)
            delete_files.append(entry)

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
        "base_version": args.version,
        "upstream_pr": 8719,
        "upstream_commit": SOURCE_COMMITS[args.version],
        "source_patch_sha256": SOURCE_PATCH_SHA256[args.version],
        "official_wheel_sha256": sha256(wheel.read_bytes()),
        "console_complete": True,
        "files": files,
        "delete_files": delete_files,
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
