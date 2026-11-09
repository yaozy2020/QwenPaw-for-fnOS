#!/bin/bash
# preflight.sh — 一致性门禁
# 步骤: 1) 版本号一致性 2) 仓库残留 3) cmd/ 完整性 4) 前端源码完整性
#       5) 桌面网关入口 6) 持久 helper/薄回调 7) Gateway-only 物料

# 不使用 set -e:本脚本已有 FAIL 聚合模式(收集全部问题后统一报错)。
# set -e 会让第一条失败就中止,用户只看到一个问题、修完再跑又冒出下一个。

PROJ_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

FAIL=0

echo "========== QwenPaw preflight =========="
echo

# [1/7] 版本号一致性
echo "[1/7] 版本号一致性 ..."
if python3 "$PROJ_DIR/scripts/sync-version.py" --check; then
  echo -e "${GREEN}  -> 通过${NC}"
else
  echo -e "${RED}  -> 失败：版本号漂移${NC}"
  FAIL=1
fi
echo

# [2/7] 仓库根残留文件检查
echo "[2/7] 仓库根残留文件 ..."
RESIDUE=()
for f in "$PROJ_DIR"/*.fpk "$PROJ_DIR"/*.sha256; do
  [ -f "$f" ] && RESIDUE+=("$f")
done

if [ ${#RESIDUE[@]} -gt 0 ]; then
  echo -e "${RED}  -> 失败：发现残留构建产物，应加入 .gitignore 或删除：${NC}"
  for f in "${RESIDUE[@]}"; do
    echo "    $f"
  done
  FAIL=1
else
  echo -e "${GREEN}  -> 通过${NC}"
fi
echo

# [3/7] cmd/ 生命周期脚本完整性
echo "[3/7] cmd/ 生命周期脚本完整性 ..."
REQUIRED_CMDS=(install_init install_callback upgrade_init upgrade_callback uninstall_init uninstall_callback config_init config_callback main)
MISSING=()
for c in "${REQUIRED_CMDS[@]}"; do
  if [ ! -f "$PROJ_DIR/cmd/$c" ]; then
    MISSING+=("$c")
  fi
done

if [ ${#MISSING[@]} -gt 0 ]; then
  echo -e "${RED}  -> 失败：缺少必需的 cmd 脚本：${NC}"
  for c in "${MISSING[@]}"; do
    echo "    cmd/$c"
  done
  FAIL=1
else
  echo -e "${GREEN}  -> 通过${NC}"
fi
echo

# [4/7] 前端源码完整性(控制台由 web/src 经 esbuild 编译,源码即 SSOT;
# 产物 app/www/index.html 自 -6 起退为构建时生成,不再入库)
echo "[4/7] 前端源码完整性 ..."
REQUIRED_WEB=(web/src/index.html web/src/main.ts web/src/components.tsx web/src/statusStore.ts web/src/style.css web/build.mjs web/package.json)
MISSING_WEB=()
for f in "${REQUIRED_WEB[@]}"; do
  if [ ! -f "$PROJ_DIR/$f" ]; then
    MISSING_WEB+=("$f")
  fi
done
if [ ${#MISSING_WEB[@]} -gt 0 ]; then
  echo -e "${RED}  -> 失败：缺少前端源码：${NC}"
  for f in "${MISSING_WEB[@]}"; do
    echo "    $f"
  done
  FAIL=1
else
  echo -e "${GREEN}  -> 通过${NC}"
fi
echo

# [5/7] 桌面统一网关入口与固定 Socket 环境
echo "[5/7] 桌面网关入口与固定 Socket ..."
if python3 - "$PROJ_DIR" <<'PY'
import json
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
failures = []
try:
    config = json.loads((root / "app/ui/config").read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError) as error:
    config = {}
    failures.append(f"app/ui/config 无法解析: {error}")
urls = config.get(".url", {}) if isinstance(config, dict) else {}
expected_console = {
    "title": "QwenPaw控制台",
    "icon": "images/console_{0}.png",
    "type": "iframe",
    "protocol": "http",
    "url": "/app/qwenpaw/",
    "gatewayPrefix": "/app/qwenpaw",
    "gatewaySocket": "app.sock",
    "allUsers": False,
    "control": {"accessPerm": "editable"},
}
expected_main = {
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
if not isinstance(urls, dict) or urls.get("qwenpaw.console") != expected_console:
    failures.append("qwenpaw.console 必须使用 /app/qwenpaw/ 与 app.sock，并保留既有 iframe 权限配置")
if not isinstance(urls, dict) or urls.get("qwenpaw.main") != expected_main:
    failures.append("qwenpaw.main 桌面入口必须保持原配置")
try:
    main = (root / "cmd/main").read_text(encoding="utf-8")
except OSError as error:
    main = ""
    failures.append(f"cmd/main 无法读取: {error}")
expected_export = 'export QWENPAW_GATEWAY_SOCKET="${APPDEST}/app.sock"'
assignments = re.findall(r"(?m)^[ \t]*(?:export[ \t]+)?QWENPAW_GATEWAY_SOCKET[ \t]*=.*$", main)
if assignments != [expected_export]:
    failures.append('cmd/main 必须只固定导出 QWENPAW_GATEWAY_SOCKET="${APPDEST}/app.sock"')
if 'exec "$candidate" service "$@"' not in main:
    failures.append('cmd/main 必须继续固定 exec service 并转发参数')
if failures:
    for failure in failures:
        print(f"preflight: FAIL: {failure}", file=sys.stderr)
    raise SystemExit(1)
print("preflight: 桌面网关入口、qwenpaw.main 保持项与固定 Socket 环境通过")
PY
then
  echo -e "${GREEN}  -> 通过${NC}"
else
  echo -e "${RED}  -> 失败：桌面网关入口或固定 Socket 配置不符合契约${NC}"
  FAIL=1
fi
echo

# [6/7] 持久 helper 与薄回调边界
echo "[6/7] 持久 helper 与薄回调边界 ..."
if python3 - "$PROJ_DIR" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
failures = []


def active(path):
    return [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]


def expected_install():
    return [
        'candidate=""',
        'if [ -n "${TRIM_APPDEST:-}" ]; then',
        'for candidate_path in "$TRIM_APPDEST/app/bin/qwenpaw-supervisor" "$TRIM_APPDEST/bin/qwenpaw-supervisor"; do',
        'if [ -x "$candidate_path" ]; then',
        'candidate=$candidate_path',
        'break',
        'fi',
        'done',
        'fi',
        'if [ -z "$candidate" ]; then',
        'for candidate_path in "/var/apps/qwenpaw/target/bin/qwenpaw-supervisor" "/var/apps/qwenpaw/app/bin/qwenpaw-supervisor"; do',
        'if [ -x "$candidate_path" ]; then',
        'candidate=$candidate_path',
        'break',
        'fi',
        'done',
        'fi',
        'if [ -z "$candidate" ]; then',
        'exit 127',
        'fi',
        'exec "$candidate" platform install',
    ]


def expected_upgrade_init():
    return [
        'if [ -x "${0%/*}/qwenpaw-supervisor" ]; then',
        'exec "${0%/*}/qwenpaw-supervisor" platform upgrade-init',
        "fi",
        'APPDEST="${TRIM_APPDEST:-/var/apps/qwenpaw/target}"',
        'for candidate in \\',
        '"${APPDEST}/app/bin/qwenpaw-supervisor" \\',
        '"${APPDEST}/bin/qwenpaw-supervisor" \\',
        '"/var/apps/qwenpaw/target/bin/qwenpaw-supervisor" \\',
        '"/var/apps/qwenpaw/app/bin/qwenpaw-supervisor"',
        "do",
        'if [ -x "$candidate" ]; then',
        'exec "$candidate" platform upgrade-init',
        "fi",
        "done",
        'printf \'%s\\n\' "supervisor binary not found" >&2',
        "exit 1",
    ]


upgrade_init = root / "cmd/upgrade_init"
if not upgrade_init.is_file() or active(upgrade_init) != expected_upgrade_init():
    failures.append("cmd/upgrade_init must prefer the staged sibling then use compatible target candidates")

for name, action in (
    ("install_callback", "install"),
    ("upgrade_callback", "upgrade"),
    ("uninstall_callback", "uninstall"),
):
    path = root / "cmd" / name
    if not path.is_file():
        failures.append(f"cmd/{name} missing")
        continue
    expected = expected_install() if action == "install" else [
        'if [ -n "${TRIM_PKGVAR:-}" ] && [ -x "${TRIM_PKGVAR}/qwenpaw-supervisor-helper" ]; then',
        f'exec "${{TRIM_PKGVAR}}/qwenpaw-supervisor-helper" platform {action}',
        "fi",
        f'if [ -x "${{0%/*}}/qwenpaw-supervisor" ]; then',
        f'exec "${{0%/*}}/qwenpaw-supervisor" platform {action}',
        "fi",
        "exit 127",
    ]
    if active(path) != expected:
        failures.append(f"cmd/{name} is not the fixed thin callback/helper shape")

helper_source = root / "supervisor/src/helper.rs"
if not helper_source.is_file():
    failures.append("supervisor/src/helper.rs missing")
else:
    source = helper_source.read_text(encoding="utf-8")
    for marker in (
        'const HELPER_NAME: &str = "qwenpaw-supervisor-helper";',
        "const HELPER_MODE: u32 = 0o750;",
        ".create_new(true)",
        ".sync_all()",
        "fs::rename",
        "fs::remove_file",
    ):
        if marker not in source:
            failures.append(f"helper staging marker missing: {marker}")
    if "/var/apps/" in source:
        failures.append("helper staging must not contain a global /var/apps destination")

build_script = (root / "build.sh").read_text(encoding="utf-8")
for marker in (
    'cp "$SUP_BIN_SRC" "$STAGE/cmd/qwenpaw-supervisor"',
    'chmod 0755 "$STAGE/cmd/qwenpaw-supervisor"',
    'cp "$SUP_BIN_SRC" "$STAGE/app/bin/qwenpaw-supervisor"',
    'chmod 0755 "$STAGE/app/bin/qwenpaw-supervisor"',
):
    if build_script.count(marker) != 1:
        failures.append(f"build.sh must materialize both release supervisor copies exactly once: {marker}")

config_source = (root / "supervisor/src/config.rs").read_text(encoding="utf-8")
service_source = (root / "supervisor/src/service.rs").read_text(encoding="utf-8")
if 'join("bin/qwenpaw-supervisor")' not in config_source:
    failures.append("Config must pin the manager binary to the installed target/bin path")
if "&cfg.supervisor_binary" not in service_source or "Command::new(expected_exe)" not in service_source:
    failures.append("service lifecycle must identify and spawn the configured target binary")
if re.search(r'env::var(?:_os)?\(\s*"[^"]*(?:SUPERVISOR|MANAGER)[^"]*(?:BIN|PATH)', config_source):
    failures.append("Config must not accept a manager executable path override")

if failures:
    for failure in failures:
        print(f"preflight: FAIL: {failure}", file=sys.stderr)
    raise SystemExit(1)
print("preflight: fixed callbacks, compatible Rust helper staging, release copies and target identity passed")
PY
then
  echo -e "${GREEN}  -> 通过${NC}"
else
  echo -e "${RED}  -> 失败：helper staging 或薄回调边界不符合契约${NC}"
  FAIL=1
fi
echo

# [7/7] Gateway-only 物料与调用链
echo "[7/7] Gateway-only 物料与调用链 ..."
if python3 - "$PROJ_DIR" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
failures = []
retired = (
    "app/ui/api.cgi",
    "app/ui/index.cgi",
    "scripts/browser-cgi-gateway.py",
    "scripts/test-native-api-cgi.py",
    "scripts/test-native-index-cgi.py",
    "web/e2e/control-panel.spec.ts",
)
for rel in retired:
    if (root / rel).exists():
        failures.append(f"已退役 CGI 物料仍存在: {rel}")

active = (
    "build.sh", "scripts/lint.sh", ".github/workflows/ci.yml",
    ".github/workflows/release.yml", "web/package.json",
    "web/playwright.config.ts", "web/e2e/run.mjs", "web/e2e/README.md",
    "scripts/browser-gateway.py", "web/src/api.ts", "web/src/main.ts", "web/build.mjs",
)
retired_calls = (
    "test:e2e:cgi", "browser-cgi-gateway.py", "test-native-api-cgi.py",
    "test-native-index-cgi.py", "web api-cgi", "web index-cgi",
)
for rel in active:
    path = root / rel
    if not path.is_file():
        failures.append(f"Gateway-only 链路文件缺失: {rel}")
        continue
    content = path.read_text(encoding="utf-8")
    for call in retired_calls:
        if call in content:
            failures.append(f"{rel} 仍调用或引用已退役物料: {call}")
if failures:
    for failure in failures:
        print(f"preflight: FAIL: {failure}", file=sys.stderr)
    raise SystemExit(1)
print("preflight: CGI 入口、专用 runner 与测试已移除；活跃构建/CI/E2E 链路仅使用 Gateway")
PY
then
  echo -e "${GREEN}  -> 通过${NC}"
else
  echo -e "${RED}  -> 失败：旧 CGI 物料仍存在或仍被调用${NC}"
  FAIL=1
fi
echo

echo "========================================"
if [ $FAIL -eq 0 ]; then
  echo -e "${GREEN}preflight: ALL CHECKS PASSED${NC}"
  exit 0
else
  echo -e "${RED}preflight: FAILED${NC}"
  exit 1
fi
