#!/bin/bash
# QwenPaw FPK 打包脚本（基座重构版：agentscope-ai/QwenPaw v2.3.0-beta.1）
# 用法: bash build.sh
#   环境变量：
#     SKIP_FRONTEND=1   跳过前端构建（ui-fndesign 控制面板，须已有 app/www 产物）
#     SKIP_CONSOLE=1    跳过官方 console 构建（须已有 src/qwenpaw/console 产物）
#     CONSOLE_FORCE_INSTALL=1  强制重装 console 依赖
#
# 注意：不用 set -e（工程铁律），失败路径全部显式 exit 1

PROJ_DIR="$(cd "$(dirname "$0")" && pwd)"
STAGE="$PROJ_DIR/build/staging"
VERSION=$(grep '^version' "$PROJ_DIR/manifest" | awk -F'=' '{print $2}' | tr -d ' ')
OUTPUT="$PROJ_DIR/com.dustinky.qwenpaw_v${VERSION}.fpk"
CODE_DIR="$PROJ_DIR/app/qwenpaw/code"

# 颜色
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}===== QwenPaw FPK v${VERSION} 打包 =====${NC}"

# [0a/6] 版本号 SSOT 同步（manifest -> README badge）
echo "[0a/6] 版本号 SSOT 同步 ..."
python3 "$PROJ_DIR/scripts/sync-version.py" || {
  echo -e "${RED}ERROR: 版本号同步失败${NC}"
  exit 1
}

# [0b/6] AUDIT_REPORT 自动生成
echo "[0b/6] AUDIT_REPORT 自动生成 ..."
python3 "$PROJ_DIR/scripts/gen-audit.py" || echo -e "${YELLOW}  警告：AUDIT_REPORT 生成失败（非致命）${NC}"

# [0c/6] preflight 一致性门禁
echo "[0c/6] preflight 一致性门禁 ..."
bash "$PROJ_DIR/scripts/preflight.sh" || {
  echo -e "${RED}ERROR: preflight 失败，请修复上述问题后重试${NC}"
  exit 1
}

# [1/6] 构建前端 UI（Vue 3 + Vite + Nuxt UI）— bun 优先
if [ "${SKIP_FRONTEND:-0}" = "1" ]; then
  echo "[1/6] 构建前端 UI ... 跳过（SKIP_FRONTEND=1）"
else
  echo "[1/6] 构建前端 UI ..."
  cd "$PROJ_DIR/ui-fndesign" || exit 1

  # fnOS QwenPaw 控制台 base path
  export VITE_BASE_PATH="${VITE_BASE_PATH:-/cgi/ThirdParty/com.dustinky.qwenpaw/index.cgi/}"
  echo "  VITE_BASE_PATH=$VITE_BASE_PATH"

  if [ -x "$HOME/.bun/bin/bun" ]; then
    echo "  使用 Bun 工具链"
    "$HOME/.bun/bin/bun" install --ignore-scripts
    # workaround: bun run build 在某些环境下报 CouldntReadCurrentDirectory
    "$HOME/.bun/bin/bun" ./node_modules/vite/bin/vite.js build
  elif command -v pnpm >/dev/null 2>&1; then
    echo -e "${YELLOW}  未找到 bun，回退到 pnpm${NC}"
    pnpm install --frozen-lockfile --ignore-scripts || pnpm install --ignore-scripts
    pnpm build
  elif command -v npm >/dev/null 2>&1; then
    echo -e "${YELLOW}  未找到 bun/pnpm，回退到 npm${NC}"
    if [ -f package-lock.json ]; then npm ci --ignore-scripts; else npm install --ignore-scripts; fi
    npm run build
  else
    echo -e "${RED}ERROR: 未找到 bun / pnpm / npm，无法构建前端${NC}"
    exit 1
  fi

  # Vite outDir 已配置为 ../app/www，产物直接落在 app/www/，无需再 cp
  cd "$PROJ_DIR" || exit 1
fi

# [1b/6] 构建官方 console（React + Vite，源码在 app/qwenpaw/code/console/，
#        产物输出到 app/qwenpaw/code/src/qwenpaw/console/ —— 与 console_static.py
#        的解析顺序和官方 Dockerfile 的做法一致）
CONSOLE_SRC="$CODE_DIR/console"
CONSOLE_DEST="$CODE_DIR/src/qwenpaw/console"
if [ "${SKIP_CONSOLE:-0}" = "1" ]; then
  echo "[1b/6] 构建 console ... 跳过（SKIP_CONSOLE=1）"
  if [ ! -f "$CONSOLE_DEST/index.html" ]; then
    echo -e "${RED}ERROR: SKIP_CONSOLE=1 但 $CONSOLE_DEST/index.html 不存在${NC}"
    exit 1
  fi
else
  echo "[1b/6] 构建 QwenPaw 官方 console ..."
  if [ ! -f "$CONSOLE_SRC/package-lock.json" ]; then
    echo -e "${RED}ERROR: 未找到 $CONSOLE_SRC/package-lock.json，console 源码不完整${NC}"
    exit 1
  fi
  cd "$CONSOLE_SRC" || exit 1
  if command -v npm >/dev/null 2>&1; then
    echo "  使用 npm（官方工具链，构建含 tsc/verify/precompress，耗时较长）"
    if [ ! -d node_modules ] || [ "${CONSOLE_FORCE_INSTALL:-0}" = "1" ]; then
      npm ci --no-audit --no-fund --prefer-offline 2>&1 | tail -3
    fi
    npm run build 2>&1 | tail -5
  else
    echo -e "${RED}ERROR: 未找到 npm，无法构建 console${NC}"
    exit 1
  fi
  if [ ! -f "dist/index.html" ]; then
    echo -e "${RED}ERROR: console 构建产物 dist/index.html 不存在，构建疑似失败${NC}"
    exit 1
  fi
  rm -rf "$CONSOLE_DEST"
  mkdir -p "$CONSOLE_DEST"
  cp -a dist/. "$CONSOLE_DEST/"
  cd "$PROJ_DIR" || exit 1
  echo "  console 产物已拷贝到 $CONSOLE_DEST"
fi

# [2/6] 准备 staging（首个重构版采取保守策略：整树打包，排除纯开发目录；
#        plugins/ 与 packages/ 必须保留 —— v2.x 插件发现和 pyproject 本地依赖都依赖它们，
#        瘦身留到安装验证通过后的二期）
echo "[2/6] 准备 staging ..."
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
  --exclude='app/qwenpaw/code/.github' \
  --exclude='app/qwenpaw/code/console' \
  --exclude='app/qwenpaw/code/tests' \
  --exclude='app/qwenpaw/code/e2e' \
  --exclude='app/qwenpaw/code/website' \
  --exclude='app/qwenpaw/code/scripts' \
  --exclude='app/qwenpaw/code/docker-compose.yml' \
  --exclude='app/qwenpaw/code/Makefile' \
  --exclude='app/qwenpaw/code/design.md' \
  --exclude='app/qwenpaw/code/CONTRIBUTING.md' \
  --exclude='app/qwenpaw/code/CONTRIBUTING_zh.md' \
  --exclude='app/qwenpaw/code/RELEASING.md' \
  --exclude='app/qwenpaw/code/RELEASING_zh.md' \
  --exclude='app/qwenpaw/code/README_ja.md' \
  --exclude='app/qwenpaw/code/README_ru.md' \
  --exclude='app/qwenpaw/code/README_vi.md' \
  --exclude='app/qwenpaw/code/README_zh.md' \
  . | tar -xf - -C "$STAGE"

if [ ! -d "$STAGE/app/qwenpaw/code/src/qwenpaw" ]; then
  echo -e "${RED}ERROR: staging 缺少 src/qwenpaw，tar 复制失败${NC}"
  exit 1
fi

# [3/6] fnpack 构建
echo "[3/6] fnpack 构建 ..."
cd "$STAGE" || exit 1
fnpack build -d .

# [4/6] 产物处理（命名版本号）
echo "[4/6] 产物处理 ..."
FPK_IN_STAGE=$(ls "$STAGE"/*.fpk 2>/dev/null | head -1)
if [ -z "$FPK_IN_STAGE" ] || [ ! -f "$FPK_IN_STAGE" ]; then
  echo -e "${RED}ERROR: fnpack 产物未找到${NC}"
  exit 1
fi
mv "$FPK_IN_STAGE" "$OUTPUT"

# [5/6] 计算 SHA256
echo "[5/6] 计算 SHA256 ..."
sha256sum "$OUTPUT" > "${OUTPUT}.sha256"

# [6/6] 完成
echo ""
echo -e "${GREEN}===== 打包完成 =====${NC}"
echo "输出: $OUTPUT"
ls -lh "$OUTPUT"
echo ""
echo "SHA256:"
cat "${OUTPUT}.sha256"
