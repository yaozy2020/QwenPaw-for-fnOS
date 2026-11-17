#!/usr/bin/env python3
import argparse
import hashlib
import io
from pathlib import Path
import subprocess
import tarfile

SOURCE_COMMIT = "0a21644e90d8ba37e20035b3bfb63880872cc2a2"
SOURCE_PATCH = Path(__file__).resolve().parents[1] / "patches/qwenpaw-2.3.0-base-path-source.patch"


def main():
    parser = argparse.ArgumentParser(description="Prepare the version-matched QwenPaw 2.3.0 base-path source.")
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    patch = SOURCE_PATCH.read_bytes()
    archive = subprocess.check_output(["git", "archive", SOURCE_COMMIT], cwd=args.repository)
    output.mkdir(parents=True, exist_ok=False)
    with tarfile.open(fileobj=io.BytesIO(archive)) as source:
        for member in source.getmembers():
            if not (output / member.name).resolve().is_relative_to(output):
                raise ValueError("source archive path escapes output")
            if not member.isdir() and not member.isfile():
                raise ValueError("source archive contains a non-regular member")
        source.extractall(output)
    subprocess.run(["patch", "--batch", "--forward", "--fuzz=0", "-p1"], input=patch, cwd=output, check=True)
    print(f"source_commit={SOURCE_COMMIT}")
    print(f"source_patch_sha256={hashlib.sha256(patch).hexdigest()}")
    print(f"source={output}")


if __name__ == "__main__":
    main()
