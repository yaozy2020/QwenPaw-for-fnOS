#!/bin/bash
# preflight.sh — 一致性门禁
# 步骤: 1) 版本号一致性 2) 仓库残留 3) cmd/ 完整性

set -e

PROJ_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

FAIL=0

echo "========== QwenPaw preflight =========="
echo

# [1/3] 版本号一致性
echo "[1/3] 版本号一致性 ..."
if python3 "$PROJ_DIR/scripts/sync-version.py" --check; then
  echo -e "${GREEN}  -> 通过${NC}"
else
  echo -e "${RED}  -> 失败：版本号漂移${NC}"
  FAIL=1
fi
echo

# [2/3] 仓库根残留文件检查
echo "[2/3] 仓库根残留文件 ..."
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

# [3/3] cmd/ 生命周期脚本完整性
echo "[3/3] cmd/ 生命周期脚本完整性 ..."
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

echo "========================================"
if [ $FAIL -eq 0 ]; then
  echo -e "${GREEN}preflight: ALL CHECKS PASSED${NC}"
  exit 0
else
  echo -e "${RED}preflight: FAILED${NC}"
  exit 1
fi
