#!/usr/bin/env python3
import re
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
SHELL_PYTHON_COMMAND = re.compile(r"\bpython(?:[0-9]+(?:\.[0-9]+)*)?\b", re.IGNORECASE)
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


def supervisor_candidates(source):
    code = shell_code(source)
    loop = re.search(
        r"(?ms)^[ \t]*for[ \t]+candidate[ \t]+in[ \t]*\\[ \t]*\n(?P<items>.*?)^[ \t]*do[ \t]*$",
        code,
    )
    if loop is None:
        return None

    candidates = []
    for line in loop.group("items").splitlines():
        item = line.strip()
        if item.endswith("\\"):
            item = item[:-1].rstrip()
        match = re.fullmatch(r'"([^"\n]+)"', item)
        if match is None:
            return None
        candidates.append(match.group(1))
    return candidates or None


def api_cgi_code(candidates):
    path_lines = [
        f'    "{candidate}"' + (" \\" if index < len(candidates) - 1 else "")
        for index, candidate in enumerate(candidates)
    ]
    return [
        'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"',
        "for candidate in \\",
        *path_lines,
        "do",
        '    if [ -x "$candidate" ]; then',
        '        exec "$candidate" web api-cgi',
        "    fi",
        "done",
        'printf \'%s\\n\' "supervisor binary not found" >&2',
        "exit 1",
    ]


def inspect_api_cgi(repo_root):
    rel = "app/ui/api.cgi"
    path = repo_root / rel
    if not path.is_file():
        return [f"{rel} is required as the native CGI thin entry"]

    source = path.read_text(encoding="utf-8")
    code = shell_code(source)
    errors = []
    candidates = supervisor_candidates(source)
    main_path = repo_root / "cmd" / "main"
    main_candidates = (
        supervisor_candidates(main_path.read_text(encoding="utf-8"))
        if main_path.is_file()
        else None
    )
    if candidates is None:
        errors.append(f"{rel}: cannot resolve supervisor candidate paths")
    elif main_candidates is None or candidates != main_candidates:
        errors.append(f"{rel}: candidate paths must match cmd/main")

    if re.search(r"(?i)\baction\b", code):
        errors.append(f"{rel}: contains action dispatch")
    if re.search(r"(?i)\bcurl\b", code):
        errors.append(f"{rel}: invokes curl")
    if SHELL_PYTHON_COMMAND.search(code):
        errors.append(f"{rel}: invokes Python")
    if re.search(r"(?i)csrf", code):
        errors.append(f"{rel}: contains CSRF logic")
    pip_mirror_write = re.compile(
        r'''(?ix)(?<![<])(?:>>?|>\||&>|<>)[ \t]*(?:"[^"\n]*\bpip-mirror\b[^"\n]*"|'[^'\n]*\bpip-mirror\b[^'\n]*'|[^\s;|&]*\bpip-mirror\b[^\s;|&]*)'''
    )
    pip_mirror_mutation = re.compile(
        r"(?i)\b(?:tee|cp|mv|install|rm|unlink|truncate)\b[^\n;|&]*\bpip-mirror\b"
    )
    pip_mirror_in_place_edit = re.compile(
        r"(?i)\bsed\b[^\n;|&]*\s-i(?:\s|$)[^\n;|&]*\bpip-mirror\b"
    )
    if any(pattern.search(code) for pattern in (
        pip_mirror_write,
        pip_mirror_mutation,
        pip_mirror_in_place_edit,
    )):
        errors.append(f"{rel}: writes pip-mirror directly")

    exec_commands = re.findall(r"(?m)^[ \t]*exec[ \t]+([^\n]+)$", code)
    if exec_commands != ['"$candidate" web api-cgi']:
        errors.append(f"{rel}: executes only web api-cgi")
    if candidates is not None and main_candidates == candidates:
        expected = [line.strip() for line in api_cgi_code(candidates)]
        actual = [line.strip() for line in code.splitlines() if line.strip()]
        if actual != expected:
            errors.append(f"{rel}: must only locate the supervisor and exec web api-cgi")
    return errors


def index_cgi_code(candidates):
    return [line.replace("web api-cgi", "web index-cgi") for line in api_cgi_code(candidates)]


def inspect_index_cgi(repo_root):
    rel = "app/ui/index.cgi"
    path = repo_root / rel
    if not path.is_file():
        return [f"{rel} is required as the native CGI thin entry"]

    source = path.read_text(encoding="utf-8")
    code = shell_code(source)
    errors = []
    candidates = supervisor_candidates(source)
    main_path = repo_root / "cmd" / "main"
    main_candidates = (
        supervisor_candidates(main_path.read_text(encoding="utf-8"))
        if main_path.is_file()
        else None
    )
    if candidates is None:
        errors.append(f"{rel}: cannot resolve supervisor candidate paths")
    elif main_candidates is None or candidates != main_candidates:
        errors.append(f"{rel}: candidate paths must match cmd/main")

    exec_commands = re.findall(r"(?m)^[ \t]*exec[ \t]+([^\n]+)$", code)
    if exec_commands != ['"$candidate" web index-cgi']:
        errors.append(f"{rel}: executes only web index-cgi")
    if candidates is not None and main_candidates == candidates:
        expected = [line.strip() for line in index_cgi_code(candidates)]
        actual = [line.strip() for line in code.splitlines() if line.strip()]
        if actual != expected:
            errors.append(f"{rel}: must only locate the supervisor and exec web index-cgi")
    return errors


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
    errors.extend(inspect_api_cgi(repo_root))
    errors.extend(inspect_index_cgi(repo_root))
    return errors, blockers, notes


def exit_code(errors):
    return 1 if errors else 0


def write_fixture(src_root, rel, source):
    path = src_root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source, encoding="utf-8")


def valid_platform_entry(subcommand):
    return (
        "#!/bin/bash\n"
        "# Thin entry only; no pip, Python, curl, rm, chown, source, or case business logic.\n"
        'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n'
        'for candidate in "${APPDEST}/bin/qwenpaw-supervisor"; do\n'
        '    if [ -x "$candidate" ]; then\n'
        f'        exec "$candidate" platform {subcommand}\n'
        "    fi\n"
        "done\n"
        'printf \'%s\\n\' "supervisor binary not found" >&2\n'
        "exit 1\n"
    )


def write_valid_platform_entries(repo):
    for name, subcommand in (
        ("config_callback", "configure"),
        ("config_init", "config-init"),
        ("upgrade_init", "upgrade-init"),
    ):
        write_fixture(repo, f"cmd/{name}", valid_platform_entry(subcommand))


def api_cgi_thin_fixture(extra="", candidates=None):
    candidates = candidates or supervisor_candidates(valid_management_main())
    path_lines = "\n".join(
        f'    "{candidate}"' + (" \\" if index < len(candidates) - 1 else "")
        for index, candidate in enumerate(candidates)
    )
    return (
        "#!/bin/bash\n"
        'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n'
        "for candidate in \\\n"
        + path_lines
        + "\ndo\n"
        '    if [ -x "$candidate" ]; then\n'
        '        exec "$candidate" web api-cgi\n'
        "    fi\n"
        "done\n"
        'printf \'%s\\n\' "supervisor binary not found" >&2\n'
        "exit 1\n"
        + extra
    )


def index_cgi_thin_fixture(extra="", candidates=None):
    return api_cgi_thin_fixture(extra, candidates).replace("web api-cgi", "web index-cgi")


def write_valid_management_cmd_entries(repo):
    write_fixture(repo, "cmd/main", valid_management_main())
    write_fixture(repo, "app/ui/api.cgi", api_cgi_thin_fixture())
    write_fixture(repo, "app/ui/index.cgi", index_cgi_thin_fixture())
    write_valid_platform_entries(repo)
    for name, subcommand in (
        ("install_callback", "install"),
        ("upgrade_callback", "upgrade"),
        ("uninstall_callback", "uninstall"),
    ):
        write_fixture(repo, f"cmd/{name}", valid_platform_entry(subcommand))
    write_fixture(repo, "cmd/install_init", "#!/bin/bash\nexit 0\n")
    write_fixture(repo, "cmd/uninstall_init", "#!/bin/bash\nexit 0\n")


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
        "for candidate in \\\n"
        + path_lines
        + "\ndo\n"
        '    if [ -x "$candidate" ]; then\n'
        '        exec "$candidate" service "$@"\n'
        "    fi\n"
        "done\n"
    )


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
        ("upgrade_callback", "upgrade", 'exec "$candidate" platform install\n', "fixed platform upgrade"),
    ]
    for entry, subcommand, injected, expected in callback_violations:
        with tempfile.TemporaryDirectory(prefix="management-p3-callback-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            target = valid_platform_entry(subcommand).replace(
                'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n',
                'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n' + injected,
            )
            if expected.startswith("fixed platform"):
                target = target.replace(f"platform {subcommand}", "platform install")
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
        write_fixture(repo, "app/ui/api.cgi", api_cgi_thin_fixture())
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
        ("cmd/main is required", None),
    ]
    for expected, bad_source in main_violations:
        with tempfile.TemporaryDirectory(prefix="management-main-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_platform_entries(repo)
            if bad_source is not None:
                main_source = bad_source if expected.startswith("does not forward") else bad_source + valid_main
                write_fixture(repo, "cmd/main", main_source)
            errors, _, _ = inspect_source(src)
            assert any(expected in error for error in errors), (
                f"cmd/main violation `{expected}` must be reported: {errors}"
            )
    print(f"management-main self-test: PASS ({len(main_violations)} negative cases, scoped positive fixture)")

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
                target = target.replace('APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n',
                                        'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"\n' + injected)
            write_fixture(repo, f"cmd/{entry}", target)
            errors, _, _ = inspect_source(src)
            assert any(f"cmd/{entry}" in error and expected in error for error in errors), (
                f"{entry} violation `{expected}` must be reported: {errors}"
            )
    print(f"management-platform self-test: PASS ({len(platform_violations)} negative cases, positive fixtures)")

    api_cgi_violations = [
        ("action dispatch", 'action=$(printf status)\n', "contains action dispatch"),
        ("curl", "curl --fail https://example.invalid\n", "invokes curl"),
        ("Python", "python3 -c 'pass'\n", "invokes Python"),
        ("CSRF", "csrf_token=\"value\"\n", "contains CSRF logic"),
        (
            "mirror write",
            'printf \'%s\\n\' "$m" > "${VAR_DIR}/pip-mirror"\n',
            "writes pip-mirror directly",
        ),
        (
            "candidate mismatch",
            api_cgi_thin_fixture().replace(
                '"${APPDEST}/app/bin/qwenpaw-supervisor"',
                '"${APPDEST}/app/bin/other-supervisor"',
            ),
            "candidate paths must match cmd/main",
        ),
        (
            "wrong command",
            api_cgi_thin_fixture().replace("web api-cgi", "platform configure"),
            "executes only web api-cgi",
        ),
    ]
    for case, content, expected in api_cgi_violations:
        with tempfile.TemporaryDirectory(prefix="management-api-cgi-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            if case in {"candidate mismatch", "wrong command"}:
                fixture = content
            else:
                fixture = api_cgi_thin_fixture(content)
            write_fixture(repo, "app/ui/api.cgi", fixture)
            errors, _, _ = inspect_source(src)
            assert any("app/ui/api.cgi" in error and expected in error for error in errors), (
                f"api.cgi {case} violation `{expected}` must be reported: {errors}"
            )
    print(f"management-api-cgi self-test: PASS ({len(api_cgi_violations)} negative cases)")

    index_cgi_violations = [
        ("candidate mismatch", index_cgi_thin_fixture().replace(
            '"${APPDEST}/app/bin/qwenpaw-supervisor"',
            '"${APPDEST}/app/bin/other-supervisor"',
        ), "candidate paths must match cmd/main"),
        ("wrong command", index_cgi_thin_fixture().replace(
            "web index-cgi", "web api-cgi"
        ), "executes only web index-cgi"),
        ("awk", 'awk \'BEGIN { print "path logic" }\'\n',
         "must only locate the supervisor and exec web index-cgi"),
        ("sed", "sed -n '1p' /tmp/path\n",
         "must only locate the supervisor and exec web index-cgi"),
        ("cat", "cat /tmp/index.html\n",
         "must only locate the supervisor and exec web index-cgi"),
        ("cookie", 'cookie="qwenpaw_csrf=value"\n',
         "must only locate the supervisor and exec web index-cgi"),
        ("path business", 'REL_PATH="${REQUEST_URI#*index.cgi}"\n',
         "must only locate the supervisor and exec web index-cgi"),
    ]
    for case, content, expected in index_cgi_violations:
        with tempfile.TemporaryDirectory(prefix="management-index-cgi-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            fixture = content if case in {"candidate mismatch", "wrong command"} else index_cgi_thin_fixture(content)
            write_fixture(repo, "app/ui/index.cgi", fixture)
            errors, _, _ = inspect_source(src)
            assert any("app/ui/index.cgi" in error and expected in error for error in errors), (
                f"index.cgi {case} violation `{expected}` must be reported: {errors}"
            )
    print(f"management-index-cgi self-test: PASS ({len(index_cgi_violations)} negative cases)")

    native_cgi_violations = [
        ("python3", 'Command::new("python3")', 'Command::new("python3")'),
        ("python3.11", 'Command::new("python3.11")', 'Command::new("python3.11")'),
        ("curl", 'Command::new("curl")', 'Command::new("curl")'),
    ]
    for command, injected, expected in native_cgi_violations:
        with tempfile.TemporaryDirectory(prefix="management-native-cgi-negative-") as temp:
            src = Path(temp) / "supervisor" / "src"
            repo = src.parent.parent
            write_fixture(src, "runtime_settings.rs", "pub struct RuntimeSettings;\n")
            write_valid_management_cmd_entries(repo)
            write_fixture(repo, "app/ui/api.cgi", api_cgi_thin_fixture())
            write_fixture(src, "cgi.rs", f"fn api_cgi() {{ {injected}; }}\n")
            errors, _, _ = inspect_source(src)
            assert any("supervisor/src/cgi.rs" in error and expected in error for error in errors), (
                f"native CGI Command::new({command}) must be rejected: {errors}"
            )
    print(f"management-native-cgi self-test: PASS ({len(native_cgi_violations)} Command negatives)")
    callback_layout_self_test()
    return 0


def check_repository(src_root):
    errors, blockers, notes = inspect_source(src_root)
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
