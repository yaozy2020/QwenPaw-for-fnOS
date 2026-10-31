#!/bin/bash
# QwenPaw for fnOS 极简 Launcher 打包脚本 (对齐 DSH 零构建架构)
# 用法: bash build.sh

PROJ_DIR="$(cd "$(dirname "$0")" && pwd)"
STAGE="$PROJ_DIR/build/staging"
VERSION=$(grep '^version' "$PROJ_DIR/manifest" | awk -F'=' '{print $2}' | tr -d ' ')
# 输出路径可被环境变量覆盖（CI 用 FPK_OUT=$GITHUB_WORKSPACE）；
# 默认仍是本机下载目录，本地用法不变。
FPK_OUT="${FPK_OUT:-/vol2/1000/下载}"
mkdir -p "$FPK_OUT"
OUTPUT="${FPK_OUT}/qwenpaw_v${VERSION}.fpk"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}===== QwenPaw for fnOS v${VERSION} Launcher 打包 =====${NC}"

# [1/4] preflight 一致性门禁
# SKIP_GATES=1 时跳过（CI 场景：宿主侧已跑过 preflight+AUDIT，且容器内可能无 python3）
if [ "${SKIP_GATES:-0}" != "1" ]; then
  echo "[1/4] preflight 一致性门禁 ..."
  bash "$PROJ_DIR/scripts/preflight.sh" || {
    echo -e "${RED}ERROR: preflight 失败${NC}"
    exit 1
  }
else
  echo "[1/4] preflight 跳过（SKIP_GATES=1，已在宿主侧完成）"
fi

# [1b/4] 构建监督者(release 单二进制;无 cargo 时复用已有产物)
SUP_BIN_SRC="$PROJ_DIR/supervisor/target/release/qwenpaw-supervisor"
if command -v cargo >/dev/null 2>&1 || [ -x "$HOME/.cargo/bin/cargo" ]; then
  echo "[1b/4] 构建监督者 (cargo release) ..."
  (cd "$PROJ_DIR/supervisor" && cargo build --release 2>&1 | tail -1)
elif [ -x "$SUP_BIN_SRC" ]; then
  echo "[1b/4] 无 cargo,复用已有监督者产物"
fi
if [ ! -x "$SUP_BIN_SRC" ]; then
  echo -e "${RED}ERROR: 监督者二进制缺失(supervisor/target/release/qwenpaw-supervisor)${NC}"
  exit 1
fi

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

# 监督者二进制入包(app/bin/,运行时位于 /var/apps/qwenpaw/app/bin/)
mkdir -p "$STAGE/app/bin"
cp "$SUP_BIN_SRC" "$STAGE/app/bin/qwenpaw-supervisor"
chmod +x "$STAGE/app/bin/qwenpaw-supervisor"

# 关键校验
if [ ! -f "$STAGE/app/www/index.html" ] || [ ! -f "$STAGE/cmd/install_callback" ]; then
  echo -e "${RED}ERROR: staging 缺少核心控制台或生命周期脚本${NC}"
  exit 1
fi

# [2b/4] 生成控制台预压缩兄弟文件（app/ui/index.cgi 按 Accept-Encoding 协商发送，
#        51KB 的控制台经 brotli 后约 13KB；生成失败仅回退未压缩，不致命）
CONSOLE_HTML="$STAGE/app/www/index.html"
NODE_BIN="$(command -v node 2>/dev/null || true)"
if [ -z "$NODE_BIN" ] && [ -x /var/apps/nodejs_v24/target/bin/node ]; then
  NODE_BIN=/var/apps/nodejs_v24/target/bin/node
fi
if [ -n "$NODE_BIN" ] && [ -x "$NODE_BIN" ]; then
  "$NODE_BIN" -e 'const {readFileSync,writeFileSync}=require("fs"),{gzipSync,brotliCompressSync,constants}=require("zlib");const f=process.argv[1],b=readFileSync(f);writeFileSync(f+".gz",gzipSync(b,{level:9}));writeFileSync(f+".br",brotliCompressSync(b,{params:{[constants.BROTLI_PARAM_QUALITY]:11}}))' "$CONSOLE_HTML" \
    && echo "    预压缩: index.html.gz/.br 已生成" \
    || echo -e "${YELLOW}    警告: 预压缩生成失败，运行时回退未压缩${NC}"
elif command -v gzip >/dev/null 2>&1; then
  gzip -9 -c "$CONSOLE_HTML" > "$CONSOLE_HTML.gz" && echo "    预压缩: 无 node，仅生成 index.html.gz"
fi

# [3/4] fnpack 快速封装
echo "[3/4] fnpack 极速构建 ..."
cd "$STAGE" || exit 1

# fnpack 可执行文件探测：环境变量显式指定优先（CI 用仓库内 vendor 的静态二进制），
# 其次本机约定路径 / command -v，最后常见路径浅层查找（找不到时显式报错）
FNPACK_BIN="${FNPACK_BIN:-$(command -v fnpack 2>/dev/null || true)}"
if [ -z "$FNPACK_BIN" ]; then
  FNPACK_BIN="$(find /usr/local/bin /usr/bin /opt /app / -maxdepth 2 -type f \( -name 'fnpack' -o -name 'fnpackup' \) -perm -u+x 2>/dev/null | head -1)"
fi
if [ -z "$FNPACK_BIN" ]; then
  echo -e "${RED}ERROR: 未找到 fnpack 可执行文件（command -v / 常见路径均无）${NC}"
  exit 1
fi
echo "  fnpack: ${FNPACK_BIN}"
"$FNPACK_BIN" build -d .

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
