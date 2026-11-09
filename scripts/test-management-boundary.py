#!/usr/bin/env python3
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


BOUNDARY_REFERENCE = re.compile(r"\baxum\b|\bapi\s*::\s*AppState\b|\bAppState\b|\bAuthCache\b|\brender_snapshot\b")
PYTHON_COMMAND = re.compile(
    r'Command\s*::\s*new\s*\(\s*["\'](python(?:[0-9]+(?:\.[0-9]+)*)?)["\']\s*\)'
)
CURL_COMMAND = re.compile(r'Command\s*::\s*new\s*\(\s*["\'](curl)["\']\s*\)')
PYTHON_C_COMMAND = re.compile(
    r'(?is)\bpython3?\b.{0,500}?\b(?:std::process::)?Command\s*::\s*new\s*\([^)]*\)'
    r'.{0,200}?\.(?:arg|args)\s*\(\s*(?:\[\s*)?["\']-c["\']'
)
TEST_MODULE = re.compile(
    r"(?m)^\s*#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub\s+)?mod\s+\w+\s*\{"
)
SUPERVISOR_CTL_SOURCE = re.compile(
    r"(?im)(?:\bsource\b|\.)[ \t]+[^\n;]*\bsupervisor_ctl(?:\.sh)?\b"
)
SHELL_SOURCE_COMMAND = re.compile(
    r"(?im)(?:^[ \t]*|[;&|()][ \t]*|(?:then|do|if)[ \t]+|![ \t]*)(?:source|\.)[ \t]+"
)
REMOVED_CMD_SOURCE = re.compile(
    r"(?im)(?:^[ \t]*|[;&|()][ \t]*|(?:then|do|if)[ \t]+|![ \t]*)"
    r"(?:source|\.)[ \t]+[^\n;]*\b(?:lib\.sh|supervisor_ctl\.sh)\b"
)
REMOVED_CMD_FILES = ("lib.sh", "supervisor_ctl.sh")
MANAGEMENT_CALLBACKS = (
    ("install_callback", "install"),
    ("upgrade_callback", "upgrade"),
    ("uninstall_callback", "uninstall"),
)
NOOP_PLATFORM_ENTRIES = ("install_init", "uninstall_init")
SHELL_ENTRY_RULES = (
    ("invokes nohup", re.compile(r"\bnohup\b", re.IGNORECASE)),
    ("invokes flock", re.compile(r"\bflock\b", re.IGNORECASE)),
    ("invokes curl", re.compile(r"\bcurl\b", re.IGNORECASE)),
    ("invokes Python", re.compile(r"\bpython(?:[0-9]+(?:\.[0-9]+)*)?\b", re.IGNORECASE)),
    (
        "writes PID files",
        re.compile(r"(?i)>>?[^\n]*\bpid(?:[_-]?(?:file|path))?\b"),
    ),
    (
        "deletes PID files",
        re.compile(r"(?i)\b(?:rm|unlink)\b[^\n;|&]*\bpid(?:[_-]?(?:file|path))?\b"),
    ),
    (
        "contains case business logic",
        re.compile(r"(?im)\bcase[ \t]+[^\n;]*?[ \t]+in\b"),
    ),
)
PLATFORM_ENTRY_RULES = SHELL_ENTRY_RULES + (
    ("writes files/configuration", re.compile(r"(?m)(?<![<])(?:>>?|>\||&>|<>)(?!&[0-9])")),
    (
        "mutates files",
        re.compile(r"\b(?:tee|cp|mv|install|rm|unlink|chmod|chown|touch|truncate)\b", re.IGNORECASE),
    ),
    ("invokes sed", re.compile(r"\bsed\b", re.IGNORECASE)),
    ("invokes pip", re.compile(r"\bpip(?:[0-9]+(?:\.[0-9]+)*)?\b", re.IGNORECASE)),
    ("invokes download utility", re.compile(r"\b(?:wget|fetch)\b", re.IGNORECASE)),
    ("invokes process control", re.compile(r"\b(?:ps|pgrep|pkill|killall|kill)\b", re.IGNORECASE)),
    ("sources files", SHELL_SOURCE_COMMAND),
)
PLATFORM_EXEC_COMMAND = re.compile(r"(?m)^[ \t]*exec\b")
SERVICE_EXEC = re.compile(
    r'(?m)^[ \t]*exec[ \t]+(?:"\$(?:\{candidate\}|candidate)"|\$(?:\{candidate\}|candidate))'
    r'[ \t]+service[ \t]+(?:"\$@"|\$@)[ \t]*(?:;[ \t]*|$)'
)


def relative(path, root):
    return path.relative_to(root).as_posix()


def is_test_source(path, src_root):
    rel = Path(relative(path, src_root))
    if rel.name in {"tests.rs", "test_support.rs"} or "tests" in rel.parts:
        return True
    return "bin" in rel.parts and re.search(r"(?:^|[-_])test(?:$|[-_])", path.stem) is not None


def production_source(source):
    test_module = TEST_MODULE.search(source)
    return source[:test_module.start()] if test_module else source


def line_number(source, offset):
    return source.count("\n", 0, offset) + 1


def management_core_files(src_root):
    files = []
    module = src_root / "management.rs"
    if module.is_file():
        files.append(module)
    module_dir = src_root / "management"
    if module_dir.is_dir():
        files.extend(path for path in module_dir.rglob("*.rs") if path.is_file())
    return sorted(set(files))


def shell_code(source):
    code = []
    quote = None
    escaped = False
    comment = False
    for offset, char in enumerate(source):
        if comment:
            if char == "\n":
                comment = False
                code.append(char)
            else:
                code.append(" ")
            continue
        if escaped:
            escaped = False
            code.append(char)
            continue
        if char == "\\" and quote != "'":
            escaped = True
            code.append(char)
            continue
        if quote is not None:
            if char == quote:
                quote = None
            code.append(char)
            continue
        if char in ("'", '"', "`"):
            quote = char
            code.append(char)
            continue
        if char == "#" and (offset == 0 or source[offset - 1].isspace() or source[offset - 1] in ";|&("):
            comment = True
            code.append(" ")
            continue
        code.append(char)
    return "".join(code)


def inspect_main_entry(repo_root):
    path = repo_root / "cmd" / "main"
    if not path.is_file():
        return ["cmd/main is required but missing"]

    source = path.read_text(encoding="utf-8")
    code = shell_code(source)
    errors = []
    for match in SUPERVISOR_CTL_SOURCE.finditer(code):
        errors.append(f"cmd/main:{line_number(code, match.start())}: sources supervisor_ctl")
    for label, pattern in SHELL_ENTRY_RULES:
        for match in pattern.finditer(code):
            errors.append(f"cmd/main:{line_number(code, match.start())}: {label}")
    socket_export = re.compile(
        r'(?m)^[ \t]*export[ \t]+QWENPAW_GATEWAY_SOCKET="\$\{APPDEST\}/app\.sock"[ \t]*$'
    )
    socket_references = list(re.finditer(r"\bQWENPAW_GATEWAY_SOCKET\b", code))
    if len(socket_references) != 1 or len(socket_export.findall(code)) != 1:
        errors.append(
            'cmd/main: QWENPAW_GATEWAY_SOCKET must be exported exactly once as "${APPDEST}/app.sock"'
        )
    if not SERVICE_EXEC.search(code):
        errors.append(
            'cmd/main: does not forward service parameters through exec "$candidate" service "$@"'
        )
    return errors


def inspect_platform_entry(repo_root, entry, subcommand):
    rel = f"cmd/{entry}"
    path = repo_root / rel
    if not path.is_file():
        return [f"{rel} is required but missing"]

    code = shell_code(path.read_text(encoding="utf-8"))
    errors = []
    for label, pattern in PLATFORM_ENTRY_RULES:
        for match in pattern.finditer(code):
            if label == "mutates files" and match.group(0).lower() == "install":
                prefix = code[max(0, match.start() - 32) : match.start()]
                if re.search(r"\bplatform[ \t]+$", prefix):
                    continue
            errors.append(f"{rel}:{line_number(code, match.start())}: {label}")

    execs = list(PLATFORM_EXEC_COMMAND.finditer(code))
    helper_literal = "qwenpaw-supervisor-helper"
    active = [line.strip() for line in code.splitlines() if line.strip()]
    if entry in {"upgrade_callback", "uninstall_callback"}:
        expected = [
            'if [ -n "${TRIM_PKGVAR:-}" ] && [ -x "${TRIM_PKGVAR}/qwenpaw-supervisor-helper" ]; then',
            f'exec "${{TRIM_PKGVAR}}/qwenpaw-supervisor-helper" platform {subcommand}',
            "fi",
            'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then',
            f'exec "${{0%/*}}/qwenpaw-supervisor" platform {subcommand}',
            "fi",
            "exit 127",
        ]
        if len(execs) != 2 or active != expected:
            offset = execs[0].start() if execs else 0
            errors.append(
                f"{rel}:{line_number(code, offset)}: must use TRIM_PKGVAR helper then sibling for fixed platform {subcommand}"
            )
    elif entry == "upgrade_init":
        expected = [
            'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then',
            'exec "${0%/*}/qwenpaw-supervisor" platform upgrade-init',
            "fi",
            'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"',
            "for candidate in " + chr(92),
            '"${APPDEST}/app/bin/qwenpaw-supervisor" ' + chr(92),
            '"${APPDEST}/bin/qwenpaw-supervisor" ' + chr(92),
            '"/var/apps/qwenpaw/target/bin/qwenpaw-supervisor" ' + chr(92),
            '"/var/apps/qwenpaw/app/bin/qwenpaw-supervisor"',
            "do",
            'if [ -x "$candidate" ]; then',
            'exec "$candidate" platform upgrade-init',
            "fi",
            "done",
            'printf \'%s\\n\' "supervisor binary not found" >&2',
            "exit 1",
        ]
        if len(execs) != 2 or active != expected:
            offset = execs[0].start() if execs else 0
            errors.append(
                f"{rel}:{line_number(code, offset)}: must prioritize staged sibling over target platform upgrade-init candidates"
            )
    else:
        fixed_exec = re.compile(
            r'(?m)^[ \t]*exec[ \t]+(?:"\$(?:\{candidate\}|candidate)"|\$(?:\{candidate\}|candidate))'
            + r"[ \t]+platform[ \t]+"
            + re.escape(subcommand)
            + r"[ \t]*(?:;[ \t]*|$)"
        )
        fixed_execs = [match for match in execs if fixed_exec.match(code, match.start())]
        if len(execs) != 1 or len(fixed_execs) != 1:
            offset = execs[0].start() if execs else 0
            errors.append(
                f"{rel}:{line_number(code, offset)}: must use exactly one fixed platform {subcommand} exec"
            )
        if entry == "install_callback" and helper_literal in code:
            errors.append(f"{rel}: install callback must not use a persistent helper fallback")
    return errors


def inspect_removed_cmd_layout(repo_root):
    cmd_root = repo_root / "cmd"
    errors = []
    for name in REMOVED_CMD_FILES:
        path = cmd_root / name
        if path.exists():
            errors.append(f"cmd/{name} must be removed from production cmd")

    if not cmd_root.is_dir():
        return errors + ["cmd directory is required"]
    for path in sorted(cmd_root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(repo_root).as_posix()
        code = shell_code(path.read_text(encoding="utf-8"))
        for match in REMOVED_CMD_SOURCE.finditer(code):
            errors.append(
                f"{rel}:{line_number(code, match.start())}: sources removed management library"
            )
    return errors


def inspect_noop_entry(repo_root, entry):
    rel = f"cmd/{entry}"
    path = repo_root / rel
    if not path.is_file():
        return [f"{rel} is required as an explicit no-op"]

    code = shell_code(path.read_text(encoding="utf-8"))
    statements = [line.strip() for line in code.splitlines() if line.strip()]
    if statements != ["exit 0"]:
        return [f"{rel} must remain an explicit no-op (only `exit 0` is allowed)"]
    return []


def inspect_source(src_root):
    errors = []
    blockers = []
    notes = []
    runtime_settings = src_root / "runtime_settings.rs"
    if not runtime_settings.is_file():
        errors.append("supervisor/src/runtime_settings.rs: required boundary target is missing")
        boundary_files = []
    else:
        boundary_files = [runtime_settings]
    core_files = management_core_files(src_root)
    boundary_files.extend(core_files)
    if not core_files:
        notes.append("management core not present; its boundary check activates when src/management.rs or src/management/ is added")

    for path in boundary_files:
        source = path.read_text(encoding="utf-8")
        rel = relative(path, src_root)
        for match in BOUNDARY_REFERENCE.finditer(source):
            errors.append(
                f"supervisor/src/{rel}:{line_number(source, match.start())}: "
                f"management boundary references forbidden `{match.group(0)}`"
            )
        prod = production_source(source)
        direct_python = list(PYTHON_COMMAND.finditer(prod))
        direct_lines = set()
        for match in direct_python:
            line = line_number(prod, match.start())
            direct_lines.add(line)
            errors.append(
                f"supervisor/src/{rel}:{line}: production management code invokes "
                f'Command::new("{match.group(1)}")'
            )
        for match in CURL_COMMAND.finditer(prod):
            errors.append(
                f"supervisor/src/{rel}:{line_number(prod, match.start())}: "
                f'production management code invokes Command::new("{match.group(1)}")'
            )
        for match in PYTHON_C_COMMAND.finditer(prod):
            line = line_number(prod, match.start())
            if line not in direct_lines:
                errors.append(
                    f"supervisor/src/{rel}:{line}: management core invokes Python with `-c`"
                )

    for path in sorted(src_root.rglob("*.rs")):
        if is_test_source(path, src_root) or path in boundary_files:
            continue
        source = production_source(path.read_text(encoding="utf-8"))
        rel = relative(path, src_root)
        for label, pattern in (("Python", PYTHON_COMMAND), ("curl", CURL_COMMAND)):
            for match in pattern.finditer(source):
                errors.append(
                    f"supervisor/src/{rel}:{line_number(source, match.start())}: "
                    f'production code invokes forbidden Command::new("{match.group(1)}") ({label})'
                )

    repo_root = src_root.parent.parent
    errors.extend(inspect_removed_cmd_layout(repo_root))
    errors.extend(inspect_main_entry(repo_root))
    for entry, subcommand in MANAGEMENT_CALLBACKS + (
        ("config_callback", "configure"),
        ("config_init", "config-init"),
        ("upgrade_init", "upgrade-init"),
    ):
        errors.extend(inspect_platform_entry(repo_root, entry, subcommand))
    for entry in NOOP_PLATFORM_ENTRIES:
        errors.extend(inspect_noop_entry(repo_root, entry))
    return errors, blockers, notes


def exit_code(errors):
    return 1 if errors else 0


def write_fixture(src_root, rel, source):
    path = src_root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8")


def valid_platform_entry(subcommand):
    if subcommand in {"upgrade", "uninstall"}:
        return (
            "#!/bin/sh\n"
            'if [ -n "${TRIM_PKGVAR:-}" ] && [ -x "${TRIM_PKGVAR}/qwenpaw-supervisor-helper" ]; then\n'
            f'exec "${{TRIM_PKGVAR}}/qwenpaw-supervisor-helper" platform {subcommand}\n'
            "fi\n"
            'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then\n'
            f'exec "${{0%/*}}/qwenpaw-supervisor" platform {subcommand}\n'
            "fi\n"
            "exit 127\n"
        )
    if subcommand == "upgrade-init":
        return (
            "#!/bin/bash\n"
            'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then\n'
            '    exec "${0%/*}/qwenpaw-supervisor" platform upgrade-init\n'
            "fi\n"
            'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n'
            "for candidate in \\\n"
            '    "${APPDEST}/app/bin/qwenpaw-supervisor" \\\n'
            '    "${APPDEST}/bin/qwenpaw-supervisor" \\\n'
            '    "/var/apps/qwenpaw/target/bin/qwenpaw-supervisor" \\\n'
            '    "/var/apps/qwenpaw/app/bin/qwenpaw-supervisor"\n'
            "do\n"
            '    if [ -x "$candidate" ]; then\n'
            '        exec "$candidate" platform upgrade-init\n'
            "    fi\n"
            "done\n"
            'printf \'%s\\n\' "supervisor binary not found" >&2\n'
            "exit 1\n"
        )
    source = (
        "#!/bin/bash\n"
        'candidate=""\n'
        'if [ -n "${TRIM_APPDEST:-}" ]; then\n'
        '    for candidate_path in "$TRIM_APPDEST/app/bin/qwenpaw-supervisor" '
        '"$TRIM_APPDEST/bin/qwenpaw-supervisor"; do\n'
        '        if [ -x "$candidate_path" ]; then\n'
        '            candidate=$candidate_path\n'
        '            break\n'
        '        fi\n'
        '    done\n'
        'fi\n'
        'if [ -z "$candidate" ]; then\n'
        '    for candidate_path in "/var/apps/qwenpaw/target/bin/qwenpaw-supervisor" '
        '"/var/apps/qwenpaw/app/bin/qwenpaw-supervisor"; do\n'
        '        if [ -x "$candidate_path" ]; then\n'
        '            candidate=$candidate_path\n'
        '            break\n'
        '        fi\n'
        '    done\n'
        'fi\n'
        'if [ -z "$candidate" ]; then\n'
        '    exit 127\n'
        "fi\n"
    )
    return source + f'exec "$candidate" platform {subcommand}\n'


def write_valid_platform_entries(repo):
    for name, subcommand in (
        ("config_callback", "configure"),
        ("config_init", "config-init"),
        ("upgrade_init", "upgrade-init"),
    ):
        write_fixture(repo, f"cmd/{name}", valid_platform_entry(subcommand))


def valid_management_main():
    candidates = (
        '${APPDEST}/app/bin/qwenpaw-supervisor',
        '${APPDEST}/bin/qwenpaw-supervisor',
        '/var/apps/qwenpaw/target/bin/qwenpaw-supervisor',
        '/var/apps/qwenpaw/app/bin/qwenpaw-supervisor',
    )
    path_lines = "\n".join(
        f'    "{candidate}"' + (" \\" if index < len(candidates) - 1 else "")
        for index, candidate in enumerate(candidates)
    )
    return (
        "#!/bin/bash\n"
        'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n'
        'export QWENPAW_GATEWAY_SOCKET="${APPDEST}/app.sock"\n'
        "for candidate in \\\n"
        + path_lines
        + "\ndo\n"
        '    if [ -x "$candidate" ]; then\n'
        '        exec "$candidate" service "$@"\n'
        "    fi\n"
        "done\n"
    )


def write_valid_management_cmd_entries(repo):
    write_fixture(repo, "cmd/main", valid_management_main())
    write_valid_platform_entries(repo)
    for name, subcommand in (
        ("install_callback", "install"),
        ("upgrade_callback", "upgrade"),
        ("uninstall_callback", "uninstall"),
    ):
        write_fixture(repo, f"cmd/{name}", valid_platform_entry(subcommand))
    write_fixture(repo, "cmd/install_init", "#!/bin/bash\nexit 0\n")
    write_fixture(repo, "cmd/uninstall_init", "#!/bin/bash\nexit 0\n")


def callback_layout_self_test():
    with tempfile.TemporaryDirectory(prefix="management-p3-positive-") as temp:
        src = Path(temp) / "supervisor" / "src"
        repo = src.parent.parent
        write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
        write_valid_management_cmd_entries(repo)
        write_fixture(repo, "scripts/engineering-helper.sh", "python3 -m pip install demo\ncurl example.invalid\nrm -rf demo\nchown app:app demo\n")
        errors, _, _ = inspect_source(src)
        assert not errors, f"thin callbacks, no-op exceptions, and engineering scripts should pass: {errors}"

    for obsolete in ("lib.sh", "supervisor_ctl.sh"):
        with tempfile.TemporaryDirectory(prefix="management-p3-obsolete-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            write_fixture(repo, f"cmd/{obsolete}", "#!/bin/bash\nexit 0\n")
            errors, _, _ = inspect_source(src)
            assert any(f"cmd/{obsolete}" in error and "must be removed" in error for error in errors), (
                f"obsolete production library cmd/{obsolete} must be rejected: {errors}"
            )

    callback_violations = [
        ("install_callback", "install", "curl --fail https://example.invalid\n", "invokes curl"),
        ("upgrade_callback", "upgrade", "python3 -c 'pass'\n", "invokes Python"),
        ("uninstall_callback", "uninstall", "pip install qwenpaw\n", "invokes pip"),
        ("install_callback", "install", "rm -rf /tmp/data\n", "mutates files"),
        ("upgrade_callback", "upgrade", "chown -R app:app /tmp/data\n", "mutates files"),
        ("uninstall_callback", "uninstall", ". ./lib.sh\n", "sources files"),
        ("upgrade_callback", "upgrade", "source ./helper.sh\n", "sources files"),
        ("install_callback", "install", 'case "$1" in install) : ;; esac\n', "case business logic"),
        ("upgrade_callback", "upgrade", 'exec "$candidate" platform install\n', "TRIM_PKGVAR helper then sibling"),
        ("uninstall_callback", "uninstall", 'candidate="$TRIM_PKGVAR/qwenpaw-supervisor-helper"\n', "TRIM_PKGVAR helper then sibling"),
        ("upgrade_callback", "upgrade", 'true\n', "TRIM_PKGVAR helper then sibling"),
    ]
    for entry, subcommand, injected, expected in callback_violations:
        with tempfile.TemporaryDirectory(prefix="management-p3-callback-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            target = valid_platform_entry(subcommand).replace("\n", "\n" + injected, 1)
            write_fixture(repo, f"cmd/{entry}", target)
            errors, _, _ = inspect_source(src)
            assert any(f"cmd/{entry}" in error and expected in error for error in errors), (
                f"{entry} violation `{expected}` must be reported: {errors}"
            )

    for entry, content in (
        ("cmd/install_init", "#!/bin/bash\nprintf 'not a no-op\\n'\nexit 0\n"),
        ("cmd/uninstall_init", "#!/bin/bash\nrm -rf /tmp/data\nexit 0\n"),
    ):
        with tempfile.TemporaryDirectory(prefix="management-p3-init-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            write_fixture(repo, entry, content)
            errors, _, _ = inspect_source(src)
            assert any(entry in error and "explicit no-op" in error for error in errors), (
                f"{entry} must remain an explicit no-op: {errors}"
            )

    for obsolete in ("lib.sh", "supervisor_ctl.sh"):
        with tempfile.TemporaryDirectory(prefix="management-p3-source-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            write_fixture(repo, "cmd/main", valid_management_main() + f'. "$(dirname "$0")/{obsolete}"\n')
            errors, _, _ = inspect_source(src)
            assert any("cmd/main" in error and "sources removed" in error for error in errors), (
                f"production cmd must not source cmd/{obsolete}: {errors}"
            )

    print(
        "management-P3 self-test: PASS "
        f"({len(callback_violations)} callback negatives, 2 obsolete-file negatives, "
        "2 source-reference negatives, 2 init no-op negatives, engineering-scope positive)"
    )


def gateway_socket_runtime_self_test():
    repo_root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="management-main-gateway-socket-") as temp:
        appdest = Path(temp)
        supervisor = appdest / "bin/qwenpaw-supervisor"
        supervisor.parent.mkdir(parents=True)
        supervisor.write_text(
            "#!/bin/sh\nprintf '%s\\n' \"$QWENPAW_GATEWAY_SOCKET\" \"$@\"\n"
            "sh -c 'printf \"%s\\n\" \"$QWENPAW_GATEWAY_SOCKET\"'\n",
            encoding="utf-8",
        )
        supervisor.chmod(0o755)
        env = os.environ.copy()
        env["TRIM_APPDEST"] = str(appdest)
        env["QWENPAW_GATEWAY_SOCKET"] = str(appdest / "ignored.sock")
        for action in ("start", "status"):
            result = subprocess.run(
                ["bash", str(repo_root / "cmd/main"), action, "--probe"],
                env=env,
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            expected = f"{appdest}/app.sock\nservice\n{action}\n--probe\n{appdest}/app.sock\n"
            assert result.returncode == 0, f"cmd/main runtime probe failed: {result.stderr}"
            assert result.stdout == expected, (
                f"cmd/main must pass fixed Socket environment and service argv: {result.stdout!r}"
            )
    print("management-main gateway socket runtime self-test: PASS (start/status argv, fixed env, child inheritance)")


def self_test():
    valid_main = valid_management_main()
    with tempfile.TemporaryDirectory(prefix="management-boundary-positive-") as temp:
        src = Path(temp) / "supervisor" / "src"
        repo = src.parent.parent
        write_fixture(
            src,
            "runtime_settings.rs",
            'pub struct RuntimeSettings;\n#[cfg(test)]\nmod tests {\n'
            'fn helper() { Command::new("python3").arg("-c"); }\n}\n',
        )
        write_fixture(src, "management/core.rs", "pub fn apply() {}\n")
        write_fixture(src, "api/installation.rs", "pub fn fetch_versions() {}\n")
        write_fixture(src, "install/tests.rs", 'Command::new("python3").arg("-c");\n')
        write_fixture(src, "bin/qwenpaw-test-runtime.rs", 'Command::new("python3").arg("-c");\n')
        write_valid_management_cmd_entries(repo)
        write_fixture(repo, "cmd/main", valid_main + "# curl python3 nohup flock case supervisor_ctl source cmd/lib.sh\n")
        errors, blockers, _ = inspect_source(src)
        assert exit_code(errors) == 0, f"allowed fixture should pass: {errors}"
        assert not blockers, f"completed native Simple Index must not remain a blocker: {blockers}"

    violations = [
        ("runtime_settings.rs", "use axum::Router;\n"),
        ("runtime_settings.rs", "use crate::api::AppState;\n"),
        ("runtime_settings.rs", "use crate::api::{AppState, AuthCache};\n"),
        ("runtime_settings.rs", "AuthCache::default();\n"),
        ("management.rs", "use crate::api::AppState;\n"),
        ("management/core.rs", "render_snapshot(&state);\n"),
        ("management/core.rs", 'Command::new("python3").arg("-c").arg(script);\n'),
        (
            "management/core.rs",
            'let bin = "/usr/bin/python3"; let mut cmd = Command::new(bin); cmd.arg("-c");\n',
        ),
        ("api/status.rs", 'Command::new("python3").arg("-c").arg(script);\n'),
        ("api/installation.rs", 'Command::new("python3");\n'),
    ]
    for rel, source in violations:
        with tempfile.TemporaryDirectory(prefix="management-boundary-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_fixture(repo, "cmd/main", valid_main)
            write_valid_platform_entries(repo)
            write_fixture(src, rel, source)
            errors, _, _ = inspect_source(src)
            assert exit_code(errors) == 1, f"violation in {rel} must fail: {source!r}"
    print(f"management-boundary Rust self-test: PASS ({len(violations)} negative cases, positive fixture)")

    main_violations = [
        ("sources supervisor_ctl", '. "$ROOT/cmd/supervisor_ctl.sh"\n'),
        ("invokes nohup", 'nohup "$candidate" &\n'),
        ("invokes flock", "flock 9\n"),
        ("writes PID files", 'printf \'%s\\n\' "$$" > "$PID_FILE"\n'),
        ("deletes PID files", 'rm -f -- "$PID_FILE"\n'),
        ("invokes curl", "curl --fail https://example.invalid\n"),
        ("invokes Python", "python3 -c 'pass'\n"),
        ("contains case business logic", 'case "$1" in start) : ;; esac\n'),
        (
            'does not forward service parameters through exec "$candidate" service "$@"',
            valid_main.replace('exec "$candidate" service "$@"', 'exec "$candidate" service "$1"'),
        ),
        (
            'QWENPAW_GATEWAY_SOCKET must be exported exactly once',
            valid_main.replace('export QWENPAW_GATEWAY_SOCKET="${APPDEST}/app.sock"\n', ""),
        ),
        (
            'QWENPAW_GATEWAY_SOCKET must be exported exactly once',
            valid_main.replace('export QWENPAW_GATEWAY_SOCKET="${APPDEST}/app.sock"',
                               'export QWENPAW_GATEWAY_SOCKET="${TRIM_APPDEST}/other.sock"'),
        ),
        ("cmd/main is required", None),
    ]
    for expected, bad_source in main_violations:
        with tempfile.TemporaryDirectory(prefix="management-main-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_platform_entries(repo)
            if bad_source is not None:
                main_source = (
                    bad_source
                    if expected.startswith(("does not forward", "QWENPAW_GATEWAY_SOCKET"))
                    else bad_source + valid_main
                )
                write_fixture(repo, "cmd/main", main_source)
            errors, _, _ = inspect_source(src)
            assert any(expected in error for error in errors), (
                f"cmd/main violation `{expected}` must be reported: {errors}"
            )
    print(f"management-main self-test: PASS ({len(main_violations)} negative cases, scoped positive fixture)")
    gateway_socket_runtime_self_test()

    platform_violations = [
        ("config_callback", "configure", "curl --fail https://example.invalid\n", "invokes curl"),
        ("config_init", "config-init", "python3 -c 'pass'\n", "invokes Python"),
        (
            "upgrade_init",
            "upgrade-init",
            'printf \'%s\\n\' value > "$CONFIG_FILE"\n',
            "writes files/configuration",
        ),
        (
            "upgrade_init",
            "upgrade-init",
            'exec "$candidate" platform upgrade-init\n',
            "must prioritize staged sibling",
        ),
        (
            "config_callback",
            "configure",
            'case "$1" in configure) : ;; esac\n',
            "contains case business logic",
        ),
        ("config_init", "config-init", 'exec "$candidate" platform configure\n', "fixed platform config-init"),
    ]
    for entry, subcommand, injected, expected in platform_violations:
        with tempfile.TemporaryDirectory(prefix="management-platform-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_fixture(repo, "cmd/main", valid_main)
            write_valid_platform_entries(repo)
            target = valid_platform_entry(subcommand)
            if expected.startswith("fixed platform"):
                target = target.replace(f"platform {subcommand}", "platform configure")
            else:
                target = target.replace("#!/bin/bash\n", "#!/bin/bash\n" + injected)
            write_fixture(repo, f"cmd/{entry}", target)
            errors, _, _ = inspect_source(src)
            assert any(f"cmd/{entry}" in error and expected in error for error in errors), (
                f"{entry} violation `{expected}` must be reported: {errors}"
            )
    print(f"management-platform self-test: PASS ({len(platform_violations)} negative cases, positive fixtures)")

    callback_layout_self_test()
    gateway_only_self_test()
    return 0


LEGACY_CGI_FILES = (
    "app/ui/api.cgi",
    "app/ui/index.cgi",
    "scripts/browser-cgi-gateway.py",
    "scripts/test-native-api-cgi.py",
    "scripts/test-native-index-cgi.py",
    "web/e2e/control-panel.spec.ts",
)
GATEWAY_ONLY_FILES = (
    "build.sh",
    "scripts/lint.sh",
    ".github/workflows/ci.yml",
    ".github/workflows/release.yml",
    "web/package.json",
    "web/playwright.config.ts",
    "web/e2e/run.mjs",
    "web/e2e/README.md",
    "web/e2e/gateway.spec.ts",
    "web/src/api.ts",
    "web/src/main.ts",
    "web/build.mjs",
    "scripts/browser-gateway.py",
    "scripts/test-gateway-uds.py",
    "scripts/test-supervisor-api.py",
)
LEGACY_CGI_CALLS = (
    "app/ui/api.cgi",
    "app/ui/index.cgi",
    "scripts/browser-cgi-gateway.py",
    "scripts/test-native-api-cgi.py",
    "scripts/test-native-index-cgi.py",
    "web/e2e/control-panel.spec.ts",
    "test:e2e:cgi",
    "web api-cgi",
    "web index-cgi",
)
WORKFLOW_CGI_COMMANDS = (
    "test:e2e:cgi",
    "python3 scripts/test-native-api-cgi.py",
    "python3 scripts/test-native-index-cgi.py",
    "scripts/browser-cgi-gateway.py",
)


def inspect_gateway_only(repo_root):
    errors = []
    for rel in LEGACY_CGI_FILES:
        if (repo_root / rel).exists():
            errors.append(f"{rel} must be removed from the Gateway-only tree")
    for rel in GATEWAY_ONLY_FILES:
        path = repo_root / rel
        if not path.is_file():
            errors.append(f"{rel} is required for the Gateway-only delivery path")
            continue
        source = path.read_text(encoding="utf-8")
        calls = WORKFLOW_CGI_COMMANDS if rel.startswith(".github/workflows/") else LEGACY_CGI_CALLS
        for call in calls:
            if call in source:
                errors.append(f"{rel} still references retired CGI material `{call}`")
    return errors


def gateway_only_self_test():
    with tempfile.TemporaryDirectory(prefix="management-gateway-only-") as temp:
        repo = Path(temp)
        for rel in GATEWAY_ONLY_FILES:
            path = repo / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("Gateway-only fixture\n", encoding="utf-8")
        assert not inspect_gateway_only(repo)
        old_entry = repo / LEGACY_CGI_FILES[0]
        old_entry.parent.mkdir(parents=True, exist_ok=True)
        old_entry.write_text("legacy\n", encoding="utf-8")
        errors = inspect_gateway_only(repo)
        assert any(LEGACY_CGI_FILES[0] in error for error in errors), errors
        old_entry.unlink()
        (repo / "build.sh").write_text("cp app/ui/api.cgi stage\n", encoding="utf-8")
        errors = inspect_gateway_only(repo)
        assert any("build.sh" in error and LEGACY_CGI_FILES[0] in error for error in errors), errors
    print("management-Gateway-only self-test: PASS (retired artifacts absent; build references rejected)")


def check_repository(src_root):
    errors, blockers, notes = inspect_source(src_root)
    errors.extend(inspect_gateway_only(src_root.parent.parent))
    for note in notes:
        print(f"[INFO] {note}")
    for blocker in blockers:
        print(f"[BLOCKER] {blocker}")
    for error in errors:
        print(f"[FAIL] {error}")
    if errors:
        print(f"management-boundary: FAIL ({len(errors)} violation(s), {len(blockers)} follow-up blocker(s))")
    else:
        print(f"management-boundary: PASS ({len(blockers)} follow-up blocker(s) remain)")
    return exit_code(errors)


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    if argv:
        print("usage: test-management-boundary.py [--self-test]", file=sys.stderr)
        return 2
    repo_root = Path(__file__).resolve().parent.parent
    return check_repository(repo_root / "supervisor" / "src")


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
