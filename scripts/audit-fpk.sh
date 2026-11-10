#!/usr/bin/env bash
# audit-fpk.sh — 验证候选 FPK 的白名单、入口边界、执行位、架构和摘要。
# 用法: bash scripts/audit-fpk.sh /path/to/output.fpk
set -u

FPK="${1:-}"
if [ "$FPK" != "--self-test" ] && { [ -z "$FPK" ] || [ ! -f "$FPK" ]; }; then
    echo "audit: 用法 audit-fpk.sh <fpk路径> | --self-test" >&2
    exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "$FPK" "$ROOT" <<'PY'
import hashlib
import io
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import zlib

fpk_path, root = sys.argv[1:]
failures = []
MAX_EXPANDED = 128 * 1024 * 1024


def report(condition, success, failure):
    if condition:
        print(f"    [ OK ] {success}")
    else:
        print(f"    [FAIL] {failure}")
        failures.append(failure)


outer_files = {
    "app.tgz", "LICENSE", "ICON.PNG", "ICON_256.PNG", "manifest",
    "config/privilege", "config/resource",
    "wizard/config", "wizard/install", "wizard/uninstall", "wizard/upgrade",
    "cmd/main", "cmd/install_init", "cmd/install_callback",
    "cmd/upgrade_init", "cmd/upgrade_callback", "cmd/uninstall_init",
    "cmd/uninstall_callback", "cmd/config_init", "cmd/config_callback",
    "cmd/qwenpaw-supervisor",
}
outer_dirs = {"config", "wizard", "cmd"}
app_files = {
    "bin/qwenpaw-supervisor", "config/privilege", "config/resource",
    "ui/config",
    "ui/images/console_64.png", "ui/images/console_256.png",
    "ui/images/icon_64.png", "ui/images/icon_256.png",
    "www/index.html", "www/index.html.gz", "www/index.html.br",
}
app_dirs = {"bin", "config", "ui", "ui/images", "www"}


def safe_members(archive, expected_files, expected_dirs, label, executable_files=()):
    files = {}
    dirs = set()
    seen = set()
    expanded = 0
    members = archive.getmembers()
    if len(members) > 512:
        raise ValueError(f"{label}: excessive archive member count ({len(members)})")
    for member in members:
        raw_name = member.name
        if raw_name.endswith("/"):
            if not member.isdir():
                raise ValueError(f"{label}: non-directory path has trailing slash: {raw_name!r}")
            name = raw_name[:-1]
        else:
            name = raw_name
        parts = name.split("/")
        if (not name or name.startswith("/") or "\\" in name
                or any(part in ("", ".", "..") for part in parts)):
            raise ValueError(f"{label}: unsafe archive path {raw_name!r}")
        if name in seen:
            raise ValueError(f"{label}: duplicate archive member {name!r}")
        seen.add(name)
        mode = member.mode
        if mode & 0o7000:
            raise ValueError(f"{label}: setuid/setgid/sticky mode bits are not allowed: {name!r} ({mode:04o})")
        if mode & 0o022:
            raise ValueError(f"{label}: group/other writable mode is not allowed: {name!r} ({mode:04o})")
        if member.isdir():
            if mode not in (0o700, 0o755):
                raise ValueError(f"{label}: directory mode must be 0700 or 0755: {name!r} ({mode:04o})")
            dirs.add(name)
            continue
        if not member.isfile():
            raise ValueError(f"{label}: special/link member is not allowed: {name!r}")
        if name in executable_files:
            if mode != 0o755:
                raise ValueError(f"{label}: executable entry must be mode 0755: {name!r} ({mode:04o})")
        elif mode & 0o111:
            raise ValueError(f"{label}: ordinary file must not have execute bits: {name!r} ({mode:04o})")
        elif mode not in (0o600, 0o644):
            raise ValueError(f"{label}: ordinary file mode must be 0600 or 0644: {name!r} ({mode:04o})")
        if member.size < 0 or expanded + member.size > MAX_EXPANDED:
            raise ValueError(f"{label}: invalid or excessive expanded size")
        expanded += member.size
        files[name] = member
    if set(files) != expected_files:
        raise ValueError(
            f"{label}: file inventory differs; missing={sorted(expected_files - set(files))}, "
            f"unexpected={sorted(set(files) - expected_files)}"
        )
    if dirs != expected_dirs:
        raise ValueError(
            f"{label}: directory inventory differs; missing={sorted(expected_dirs - set(dirs))}, "
            f"unexpected={sorted(dirs - expected_dirs)}"
        )
    return files


def test_safe_members_modes():
    import tempfile

    cases = [
        ("accepts 0700 directories, 0755 entries, and 0600 data", {}, None),
        ("accepts 0755 directories and 0644 data", {"dir": 0o755, "data": 0o644}, None),
        ("rejects setuid", {"entry": 0o4755}, "setuid/setgid/sticky"),
        ("rejects setgid", {"dir": 0o2700}, "setuid/setgid/sticky"),
        ("rejects sticky", {"dir": 0o1700}, "setuid/setgid/sticky"),
        ("rejects group-write", {"data": 0o0664}, "group/other writable"),
        ("rejects other-write", {"data": 0o0646}, "group/other writable"),
        ("requires entries to be exactly 0755", {"entry": 0o0744}, "must be mode 0755"),
        ("rejects executable ordinary files", {"data": 0o0744}, "must not have execute bits"),
        ("rejects unreasonable directory permissions", {"dir": 0o0500}, "directory mode must be 0700 or 0755"),
    ]
    failed = 0
    for label, overrides, expected_error in cases:
        modes = {"dir": 0o700, "entry": 0o755, "data": 0o600}
        modes.update(overrides)
        error_text = ""
        try:
            with tempfile.TemporaryDirectory() as tempdir:
                archive_path = os.path.join(tempdir, "members.tar.gz")
                with tarfile.open(archive_path, "w:gz") as test_archive:
                    for name, mode in modes.items():
                        member = tarfile.TarInfo(name)
                        member.mode = mode
                        if name == "dir":
                            member.type = tarfile.DIRTYPE
                            test_archive.addfile(member)
                        else:
                            member.size = 1
                            test_archive.addfile(member, io.BytesIO(b"x"))
                with tarfile.open(archive_path, "r:gz") as test_archive:
                    safe_members(
                        test_archive, {"entry", "data"}, {"dir"},
                        "self-test", executable_files={"entry"},
                    )
        except Exception as error:
            error_text = str(error)
        accepted = not error_text
        passed = accepted if expected_error is None else (
            not accepted and expected_error in error_text
        )
        if passed:
            print(f"    [ OK ] mode self-test: {label}")
        else:
            detail = error_text or "archive was accepted"
            print(f"    [FAIL] mode self-test: {label}: {detail}")
            failed += 1
    if failed:
        print(f"audit: SELF-TEST FAILED ({failed} case(s))", file=sys.stderr)
        raise SystemExit(1)
    print(f"audit: SELF-TEST PASSED ({len(cases)} mode cases)")


def desktop_entry_checks(contents):
    try:
        desktop = json.loads(contents.decode("utf-8"))
        urls = desktop[".url"]
        console = urls["qwenpaw.console"]
        main_entry = urls["qwenpaw.main"]
        gateway_config_valid = (
            console.get("url") == "/app/qwenpaw/"
            and console.get("gatewayPrefix") == "/app/qwenpaw"
            and console.get("gatewaySocket") == "app.sock"
            and console.get("type") == "iframe"
            and console.get("allUsers") is False
            and console.get("control") == {"accessPerm": "editable"}
        )
        main_entry_valid = main_entry == {
            "title": "QwenPaw",
            "icon": "images/icon_{0}.png",
            "type": "iframe",
            "protocol": "",
            "url": "/app/qwenpaw/web/",
            "gatewayPrefix": "/app/qwenpaw",
            "gatewaySocket": "app.sock",
            "allUsers": False,
            "noDisplay": False,
            "control": {"accessPerm": "editable"},
        }
    except (UnicodeDecodeError, json.JSONDecodeError, KeyError, AttributeError, TypeError):
        return False, False
    return gateway_config_valid, main_entry_valid


def test_desktop_entries():
    with open(os.path.join(root, "app/ui/config"), "rb") as source:
        contents = source.read()
    assert desktop_entry_checks(contents) == (True, True), "source desktop gateway configuration must pass"
    cases = (
        ("url", "/cgi/ThirdParty/qwenpaw/index.cgi/"),
        ("gatewayPrefix", "/app/other"),
        ("gatewaySocket", "supervisor-control.sock"),
        ("type", "url"),
        ("allUsers", True),
        ("control", {"accessPerm": "readonly"}),
    )
    for field, value in cases:
        config = json.loads(contents)
        config[".url"]["qwenpaw.console"][field] = value
        assert desktop_entry_checks(json.dumps(config).encode()) == (False, True), field
        print(f"    [ OK ] desktop self-test: rejects console {field} drift")
    config = json.loads(contents)
    config[".url"]["qwenpaw.main"]["port"] = "18089"
    assert desktop_entry_checks(json.dumps(config).encode()) == (True, False), "main entry drift"
    for malformed in (b"not-json", b"null", b'[]', b'{".url": []}'):
        assert desktop_entry_checks(malformed) == (False, False), "malformed desktop configuration"
    print("audit: desktop SELF-TEST PASSED (source positive, 7 drift negatives, 4 malformed negatives)")


def has_elf_interpreter(binary):
    if len(binary) < 64 or binary[:6] != b"\x7fELF\x02\x01":
        return True
    phoff = struct.unpack_from("<Q", binary, 32)[0]
    phentsize, phnum = struct.unpack_from("<HH", binary, 54)
    if phentsize < 56 or phoff + phentsize * phnum > len(binary):
        return True
    return any(
        struct.unpack_from("<I", binary, phoff + index * phentsize)[0] == 3
        for index in range(phnum)
    )


def test_static_elf_checks():
    header = bytearray(64)
    header[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", header, 18, 62)
    struct.pack_into("<Q", header, 32, 64)
    struct.pack_into("<HH", header, 54, 56, 1)
    static_binary = bytes(header) + bytes(56)
    dynamic_binary = bytearray(static_binary)
    struct.pack_into("<I", dynamic_binary, 64, 3)
    assert not has_elf_interpreter(static_binary)
    assert has_elf_interpreter(bytes(dynamic_binary))
    assert has_elf_interpreter(b"not an ELF")
    print("audit: static ELF SELF-TEST PASSED (static-pie accepted, interpreter and malformed input rejected)")


if fpk_path == "--self-test":
    test_safe_members_modes()
    test_desktop_entries()
    test_static_elf_checks()
    raise SystemExit(0)


def payload(archive, members, name):
    source = archive.extractfile(members[name])
    if source is None:
        raise ValueError(f"cannot read archive member {name}")
    return source.read()


def active_lines(contents, label):
    try:
        text = contents.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError(f"{label} is not UTF-8: {error}") from error
    if not text.startswith("#!"):
        raise ValueError(f"{label} is missing a shebang")
    return [line.strip() for line in text.splitlines()
            if line.strip() and not line.lstrip().startswith("#")]


def located_wrapper(action):
    slash = chr(92)
    return [
        'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"',
        "for candidate in " + slash,
        '"${APPDEST}/app/bin/qwenpaw-supervisor" ' + slash,
        '"${APPDEST}/bin/qwenpaw-supervisor" ' + slash,
        '"/var/apps/qwenpaw/target/bin/qwenpaw-supervisor" ' + slash,
        '"/var/apps/qwenpaw/app/bin/qwenpaw-supervisor"',
        "do",
        'if [ -x "$candidate" ]; then',
        f'exec "$candidate" {action}',
        "fi",
        "done",
        'printf \'%s\\n\' "supervisor binary not found" >&2',
        "exit 1",
    ]


def upgrade_init_wrapper():
    return [
        'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then',
        'exec "${0%/*}/qwenpaw-supervisor" platform upgrade-init',
        "fi",
        *located_wrapper("platform upgrade-init"),
    ]


def platform_callback(action):
    if action in {"upgrade", "uninstall"}:
        return [
            'if [ -n "${TRIM_PKGVAR:-}" ] && [ -x "${TRIM_PKGVAR}/qwenpaw-supervisor-helper" ]; then',
            f'exec "${{TRIM_PKGVAR}}/qwenpaw-supervisor-helper" platform {action}',
            "fi",
            f'if [ -x "${{0%/*}}/qwenpaw-supervisor" ]; then',
            f'exec "${{0%/*}}/qwenpaw-supervisor" platform {action}',
            "fi",
            "exit 127",
        ]
    lines = [
        'candidate=""',
        'if [ -n "${TRIM_APPDEST:-}" ]; then',
        'for candidate_path in "$TRIM_APPDEST/app/bin/qwenpaw-supervisor" "$TRIM_APPDEST/bin/qwenpaw-supervisor"; do',
        'if [ -x "$candidate_path" ]; then',
        'candidate=$candidate_path',
        "break",
        "fi",
        "done",
        "fi",
        'if [ -z "$candidate" ]; then',
        'for candidate_path in "/var/apps/qwenpaw/target/bin/qwenpaw-supervisor" "/var/apps/qwenpaw/app/bin/qwenpaw-supervisor"; do',
        'if [ -x "$candidate_path" ]; then',
        'candidate=$candidate_path',
        "break",
        "fi",
        "done",
        "fi",
        'if [ -z "$candidate" ]; then',
        "exit 127",
        "fi",
        'exec "$candidate" platform install',
    ]
    return lines


def bounded_gzip_decompress(data, label):
    decoder = zlib.decompressobj(16 + zlib.MAX_WBITS)
    decoded = decoder.decompress(data, MAX_EXPANDED + 1)
    if len(decoded) > MAX_EXPANDED or decoder.unconsumed_tail:
        raise ValueError(f"{label} expands beyond the {MAX_EXPANDED}-byte limit")
    decoded += decoder.flush()
    if len(decoded) > MAX_EXPANDED:
        raise ValueError(f"{label} expands beyond the {MAX_EXPANDED}-byte limit")
    if not decoder.eof or decoder.unused_data:
        raise ValueError(f"{label} is truncated or has trailing data")
    return decoded


def brotli_decompress(data):
    node = shutil.which("node")
    fallback = "/var/apps/nodejs_v24/target/bin/node"
    if node is None and os.access(fallback, os.X_OK):
        node = fallback
    if node is None:
        raise RuntimeError("Brotli verification requires Node.js (node) or the fnOS nodejs_v24 runtime")
    script = (
        'const fs=require("node:fs"),z=require("node:zlib");'
        'process.stdout.write(z.brotliDecompressSync(fs.readFileSync(0),'
        '{maxOutputLength:134217729}));'
    )
    result = subprocess.run(
        [node, "-e", script], input=data, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, timeout=30, check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise ValueError(f"index.html.br cannot be decoded: {detail}")
    if len(result.stdout) > MAX_EXPANDED:
        raise ValueError("index.html.br expands beyond the size limit")
    return result.stdout


try:
    with tarfile.open(fpk_path, "r:gz") as outer:
        outer_members = safe_members(
            outer, outer_files, outer_dirs, "FPK",
            executable_files={
                "cmd/main", "cmd/install_init", "cmd/install_callback",
                "cmd/upgrade_init", "cmd/upgrade_callback", "cmd/uninstall_init",
                "cmd/uninstall_callback", "cmd/config_init", "cmd/config_callback",
                "cmd/qwenpaw-supervisor",
            },
        )
        outer_payloads = {
            name: payload(outer, outer_members, name) for name in outer_files
        }
    app_data = outer_payloads["app.tgz"]
    with tarfile.open(fileobj=io.BytesIO(app_data), mode="r:gz") as app:
        app_members = safe_members(
            app, app_files, app_dirs, "app.tgz",
            executable_files={"bin/qwenpaw-supervisor"},
        )
        app_payloads = {name: payload(app, app_members, name) for name in app_files}
except Exception as error:
    print(f"audit: FPK archive validation failed: {error}", file=sys.stderr)
    raise SystemExit(1)

print("audit: FPK archive and inventory")
report(True, "外层与 app.tgz 成员、类型、权限和路径均符合白名单", "")

try:
    manifest = outer_payloads["manifest"].decode("utf-8")
except UnicodeDecodeError as error:
    manifest = ""
    report(False, "manifest 是 UTF-8 文本", f"manifest 不是 UTF-8: {error}")


def manifest_value(key):
    matches = re.findall(rf"(?m)^{re.escape(key)}\s*=\s*([^\s#]+)", manifest)
    return matches[0] if len(matches) == 1 else None


platform = manifest_value("platform")
report(platform == "x86", "manifest platform=x86", "manifest platform 不是唯一的 x86 值")
version = manifest_value("version")
report(bool(version and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9]+)?", version)),
       f"manifest version={version}" if version else "manifest 含有效版本号",
       "manifest 缺少唯一且有效的 version")
checksum = manifest_value("checksum")
actual_checksum = hashlib.md5(app_data).hexdigest()
report(bool(checksum and checksum.lower() == actual_checksum),
       f"manifest app.tgz MD5 匹配 ({actual_checksum})",
       f"manifest app.tgz MD5 不匹配 (manifest={checksum or 'missing'}, actual={actual_checksum})")

cmd_paths = (
    "cmd/main", "cmd/install_init", "cmd/install_callback", "cmd/upgrade_init",
    "cmd/upgrade_callback", "cmd/uninstall_init", "cmd/uninstall_callback",
    "cmd/config_init", "cmd/config_callback", "cmd/qwenpaw-supervisor",
)
print("audit: lifecycle entries, Gateway static resources, and retired CGI absence")
for name in cmd_paths:
    report(bool(outer_members[name].mode & 0o111), f"{name} 执行位存在", f"{name} 缺少执行位")
for name in ("app/ui/api.cgi", "app/ui/index.cgi"):
    report(not os.path.lexists(os.path.join(root, name)), f"源码旧入口 {name} 已移除",
           f"源码仍存在已退役入口 {name}")
for name in ("ui/api.cgi", "ui/index.cgi"):
    report(name not in app_members, f"FPK 未打包 app/{name}", f"FPK 不得包含已退役 app/{name}")

gateway_config_valid, main_entry_valid = desktop_entry_checks(app_payloads["ui/config"])
report(gateway_config_valid, "qwenpaw.console 使用固定网关 URL/Prefix/Socket 并保留 iframe 权限属性",
       "qwenpaw.console 网关配置或保留属性不符合契约")
report(main_entry_valid, "qwenpaw.main 保持既有 URL 桌面入口配置",
       "qwenpaw.main 桌面入口配置发生漂移")

expected_wrappers = {
    "cmd/main": "service \"$@\"",
    "cmd/config_callback": "platform configure",
    "cmd/config_init": "platform config-init",
    "cmd/upgrade_init": "platform upgrade-init",
}
for name, command in expected_wrappers.items():
    try:
        actual = active_lines(outer_payloads[name], name)
        expected = upgrade_init_wrapper() if name == "cmd/upgrade_init" else located_wrapper(command)
        if name == "cmd/main":
            expected.insert(1, 'export QWENPAW_GATEWAY_SOCKET="${APPDEST}/app.sock"')
        matches = actual == expected
    except ValueError:
        matches = False
    report(matches, f"{name} 仅定位 supervisor 并固定 exec {command}",
           f"{name} 不符合薄入口白名单")

for name, action in (("cmd/install_callback", "install"),
                     ("cmd/upgrade_callback", "upgrade"),
                     ("cmd/uninstall_callback", "uninstall")):
    try:
        actual = active_lines(outer_payloads[name], name)
        matches = actual == platform_callback(action)
    except ValueError:
        matches = False
    report(matches, f"{name} 仅固定 exec platform {action}",
           f"{name} 含平台业务逻辑或不是固定 exec platform {action}")
for name in ("cmd/install_init", "cmd/uninstall_init"):
    try:
        actual = active_lines(outer_payloads[name], name)
        matches = actual == ["exit 0"]
    except ValueError:
        matches = False
    report(matches, f"{name} 是显式 no-op", f"{name} 不是显式 no-op")

print("audit: executable architecture and binary identity")
packaged_binaries = {
    "app/bin/qwenpaw-supervisor": (app_payloads["bin/qwenpaw-supervisor"], app_members["bin/qwenpaw-supervisor"].mode),
    "cmd/qwenpaw-supervisor": (outer_payloads["cmd/qwenpaw-supervisor"], outer_members["cmd/qwenpaw-supervisor"].mode),
}
source_binary = os.path.join(
    root, "supervisor", "target", "x86_64-unknown-linux-musl", "release", "qwenpaw-supervisor"
)
source_sha256 = None
if os.path.isfile(source_binary):
    with open(source_binary, "rb") as source:
        source_sha256 = hashlib.sha256(source.read()).hexdigest()
else:
    report(False, "release supervisor exists", f"release supervisor 不存在: {source_binary}")
packaged_hashes = {}
for name, (binary, mode) in packaged_binaries.items():
    is_x86_64 = (
        len(binary) >= 20
        and binary[:4] == b"\x7fELF"
        and binary[4] == 2
        and binary[5] == 1
        and struct.unpack_from("<H", binary, 18)[0] == 62
    )
    report(is_x86_64, f"{name} 是 ELF64 little-endian x86_64", f"{name} 不是 ELF64 little-endian x86_64")
    report(is_x86_64 and not has_elf_interpreter(binary),
           f"{name} 无 PT_INTERP，使用 musl 静态链接",
           f"{name} 含动态加载器 PT_INTERP 或 ELF 头无效，不是可移植静态产物")
    report(mode == 0o755, f"{name} 模式为 0755", f"{name} 模式不是 0755: {mode:04o}")
    packaged_hashes[name] = hashlib.sha256(binary).hexdigest()
    if source_sha256 is not None:
        report(packaged_hashes[name] == source_sha256,
               f"{name} SHA256 与 release 源一致 ({source_sha256})",
               f"{name} SHA256 与 release 源不一致 (包内={packaged_hashes[name]}, 源={source_sha256})")
report(len(set(packaged_hashes.values())) == 1,
       "外层 helper 与 target supervisor SHA256 一致", "外层 helper 与 target supervisor SHA256 不一致")
report("bin/qwenpaw-test-runtime" not in app_files,
       "包内白名单未包含测试辅助 runtime", "包内不应包含测试辅助 runtime")

print("audit: generated console compression")
html = app_payloads["www/index.html"]
html_source = os.path.join(root, "web", "dist", "index.html")
if os.path.isfile(html_source):
    with open(html_source, "rb") as source:
        expected_html = source.read()
    report(html == expected_html, "包内 index.html 与 web/dist/index.html 字节一致",
           "包内 index.html 与 web/dist/index.html 不一致")
else:
    report(False, "包内 index.html 与 web/dist/index.html 字节一致",
           f"控制台构建源不存在: {html_source}")
try:
    decoded_gzip = bounded_gzip_decompress(app_payloads["www/index.html.gz"], "index.html.gz")
    gzip_valid = decoded_gzip == html
except (OSError, ValueError, zlib.error) as error:
    gzip_valid = False
    gzip_detail = str(error)
else:
    gzip_detail = "decompressed bytes differ from index.html"
report(gzip_valid, "index.html.gz 解压后与 index.html 字节一致",
       f"index.html.gz 无效: {gzip_detail}")
try:
    decoded_brotli = brotli_decompress(app_payloads["www/index.html.br"])
    brotli_valid = decoded_brotli == html
except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
    brotli_valid = False
    brotli_detail = str(error)
else:
    brotli_detail = "decompressed bytes differ from index.html"
report(brotli_valid, "index.html.br 解压后与 index.html 字节一致",
       f"index.html.br 无效: {brotli_detail}")

with open(fpk_path, "rb") as fpk_file:
    fpk_sha256 = hashlib.sha256(fpk_file.read()).hexdigest()
if failures:
    print(f"audit: FAILED ({len(failures)} check(s))", file=sys.stderr)
    raise SystemExit(1)
print(f"audit: ALL CHECKS PASSED (FPK SHA256 {fpk_sha256})")
PY
