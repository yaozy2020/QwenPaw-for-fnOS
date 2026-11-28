#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import re
from urllib.parse import urlparse
from urllib.request import HTTPRedirectHandler, build_opener


STABLE = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:\.post(0|[1-9][0-9]*))?")


def version_key(value):
    match = STABLE.fullmatch(value)
    if not match:
        raise ValueError(f"not a stable x.y.z[.postN] version: {value!r}")
    return tuple(int(part or 0) for part in match.groups())


def feed_version(data, field):
    value = data
    for part in field.split("."):
        if not isinstance(value, dict) or part not in value:
            raise ValueError(f"upstream feed is missing {field}")
        value = value[part]
    if not isinstance(value, str):
        raise ValueError(f"upstream feed field {field} is not a string")
    version_key(value)
    return value


def current_version(path, key=None):
    text = path.read_text(encoding="utf-8").strip()
    if key is None:
        version = text
    else:
        values = [line.partition("=")[2].strip() for line in text.splitlines()
                  if not line.lstrip().startswith("#") and line.partition("=")[0].strip() == key]
        if len(values) != 1:
            raise ValueError(f"current version file must contain exactly one {key}")
        version = values[0]
    version_key(version)
    return version


def evaluate(project, current, latest, source):
    current_key = version_key(current)
    latest_key = version_key(latest)
    status = "update_available" if latest_key > current_key else "current" if latest_key == current_key else "local_ahead"
    return {
        "project": project,
        "current": current,
        "latest": latest,
        "status": status,
        "update_available": latest_key > current_key,
        "source": source,
    }


class HTTPSOnly(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urlparse(newurl).scheme != "https":
            raise ValueError("upstream feed redirected outside HTTPS")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def fetch(url):
    parsed = urlparse(url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise ValueError("upstream feed must be an HTTPS URL without credentials")
    with build_opener(HTTPSOnly()).open(url, timeout=20) as response:
        data = response.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError("upstream feed exceeds 1 MiB")
    return json.loads(data)


def main():
    parser = argparse.ArgumentParser(description="Report whether an upstream stable version needs adaptation.")
    parser.add_argument("--project", required=True)
    parser.add_argument("--feed", required=True)
    parser.add_argument("--field", required=True)
    parser.add_argument("--current-file", type=Path, required=True)
    parser.add_argument("--current-key")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    report = evaluate(args.project, current_version(args.current_file, args.current_key),
                      feed_version(fetch(args.feed), args.field), args.feed)
    rendered = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    print(rendered, end="")
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            for key in ("status", "current", "latest", "update_available"):
                output.write(f"{key}={str(report[key]).lower()}\n")


if __name__ == "__main__":
    main()
