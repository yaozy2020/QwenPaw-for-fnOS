#!/bin/bash
# QwenPaw for fnOS 极简 Launcher 打包脚本 (对齐 DSH 零构建架构)
# 用法: bash build.sh

PROJ_DIR="$(cd "$(dirname "$0")" && pwd)"
STAGE="$PROJ_DIR/build/staging"
VERSION=$(grep '^version' "$PROJ_DIR/manifest" | awk -F'=' '{print $2}' | tr -d ' ')
OUTPUT="/vol2/1000/下载/qwenpaw_v${VERSION}.fpk"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}===== QwenPaw for fnOS v${VERSION} Launcher 打包 =====${NC}"

# [1/4] preflight 一致性门禁
echo "[1/4] preflight 一致性门禁 ..."
bash "$PROJ_DIR/scripts/preflight.sh" || {
  echo -e "${RED}ERROR: preflight 失败${NC}"
  exit 1
}

# [2/4] 准备纯净 staging (排除源码，仅保留外壳、控制台、向导与脚本)
echo "[2/4] 准备 staging 纯净目录树 ..."
rm -rf "$STAGE"
mkdir -p "$STAGE"

tar -cf - -C "$PROJ_DIR" \
  --exclude='build' \
  --exclude='.git' \
  --exclude='.github' \
  --exclude='scripts' \
  --exclude='ui-fndesign' \
  --exclude='AUDIT_REPORT.md' \
  --exclude='*.fpk' \
  --exclude='*.sha256' \
  --exclude='app/qwenpaw' \
  --exclude='app/www/*.png' \
  --exclude='app/www/*.jpg' \
  --exclude='app/www/public' \
  . | tar -xf - -C "$STAGE"

# 关键校验
if [ ! -f "$STAGE/app/www/index.html" ] || [ ! -f "$STAGE/cmd/install_callback" ]; then
  echo -e "${RED}ERROR: staging 缺少核心控制台或生命周期脚本${NC}"
  exit 1
fi

# [3/4] fnpack 快速封装
echo "[3/4] fnpack 极速构建 ..."
cd "$STAGE" || exit 1
/usr/local/bin/fnpack build -d . >/dev/null 2>&1

FPK_IN_STAGE=$(ls "$STAGE"/*.fpk 2>/dev/null | head -1)
if [ -z "$FPK_IN_STAGE" ] || [ ! -f "$FPK_IN_STAGE" ]; then
  echo -e "${RED}ERROR: fnpack 构建失败，未产出 fpk 文件${NC}"
  exit 1
fi
mv "$FPK_IN_STAGE" "$OUTPUT"

# [4/4] 计算 SHA256 与归档
echo "[4/4] 计算签名与收尾 ..."
sha256sum "$OUTPUT" > "${OUTPUT}.sha256"
rm -rf "$STAGE"

echo ""
echo -e "${GREEN}===== 纯净 Launcher 打包完成 =====${NC}"
echo "输出: $OUTPUT"
ls -lh "$OUTPUT"
echo ""
echo "SHA256:"
cat "${OUTPUT}.sha256"
