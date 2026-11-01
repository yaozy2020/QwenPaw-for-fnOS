#!/bin/bash
# QwenPaw for fnOS 极简 Launcher 打包脚本 (对齐 DSH 零构建架构)
# 用法: bash build.sh
#
# 发布完整性纪律(P0/P1,勿回退):
#   1. cargo 构建失败必须终止出包 —— `cargo build 2>&1 | tail -1` 会吞掉退出码,
#      且陈旧二进制会被当成"本次构建结果"入包(先删产物再构建,存在即可信)。
#   2. staging 白名单复制 —— 不再 tar 整个项目目录;supervisor/target、web 源、
#      构建工具等绝不入包(audit-fpk.sh 兜底校验)。
#   3. 构建后自动审计 FPK 内容与执行位。

set -o pipefail

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
  echo "[1b/4] 构建监督者 (cargo release --locked) ..."
  # 先删旧产物:构建结束若文件存在,即是"本次构建"的产出(防止陈旧二进制冒充)
  rm -f "$SUP_BIN_SRC"
  # 不用管道收尾(2>&1|tail 会吞退出码);失败必须终止出包
  (cd "$PROJ_DIR/supervisor" && cargo build --locked --release) || {
    echo -e "${RED}ERROR: 监督者 cargo 构建失败,终止出包${NC}"
    exit 1
  }
elif [ -x "$SUP_BIN_SRC" ]; then
  echo -e "${YELLOW}[1b/4] 无 cargo,复用已有监督者产物(注意:无法保证为最新构建)${NC}"
fi
if [ ! -x "$SUP_BIN_SRC" ]; then
  echo -e "${RED}ERROR: 监督者二进制缺失(supervisor/target/release/qwenpaw-supervisor)${NC}"
  exit 1
fi
SUP_SHA256=$(sha256sum "$SUP_BIN_SRC" | awk '{print $1}')
echo "  监督者 SHA256: ${SUP_SHA256}"

# [2a/4] 编译控制台(esbuild:web/src → 单文件 dist/index.html,app/www 运行时契约不变)
echo "[2a/4] 编译控制台 (esbuild) ..."
NODE_BIN="$(command -v node 2>/dev/null || true)"
if [ -z "$NODE_BIN" ] && [ -x /var/apps/nodejs_v24/target/bin/node ]; then
  NODE_BIN=/var/apps/nodejs_v24/target/bin/node
fi
if [ -z "$NODE_BIN" ]; then
  echo -e "${RED}ERROR: 未找到 node,控制台编译需要${NC}"
  exit 1
fi
if [ ! -x "$PROJ_DIR/web/node_modules/.bin/esbuild" ]; then
  echo "  esbuild 未安装,执行 npm ci (npmmirror) ..."
  (cd "$PROJ_DIR/web" && env PATH="$(dirname "$NODE_BIN"):$PATH" \
    npm ci --registry=https://registry.npmmirror.com --no-audit --no-fund 2>&1 | tail -1) || {
    echo -e "${RED}ERROR: npm ci 失败${NC}"
    exit 1
  }
fi
BUILD_ID="r$(git -C "$PROJ_DIR" rev-list --count HEAD 2>/dev/null || echo 0)"
export BUILD_ID
(cd "$PROJ_DIR/web" && "$NODE_BIN" build.mjs) || {
  echo -e "${RED}ERROR: 控制台编译失败(语法/引用错误在 web/src 中)${NC}"
  exit 1
}

# [2/4] staging 白名单复制(不再 tar 整个项目 —— supervisor/target、构建工具、
#       源码、CI 配置等运行时无关文件此前会随整目录复制进入包内)
echo "[2/4] staging 白名单目录树 ..."
rm -rf "$STAGE"
mkdir -p "$STAGE"

# 顶层白名单:清单/许可/图标 + 运行时目录
cp "$PROJ_DIR/manifest" "$STAGE/manifest"
cp "$PROJ_DIR/LICENSE" "$STAGE/LICENSE"
cp "$PROJ_DIR/ICON.PNG" "$PROJ_DIR/ICON_256.PNG" "$STAGE/"
cp -a "$PROJ_DIR/app" "$STAGE/app"
cp -a "$PROJ_DIR/cmd" "$STAGE/cmd"
cp -a "$PROJ_DIR/config" "$STAGE/config"
cp -a "$PROJ_DIR/wizard" "$STAGE/wizard"

# app 内非运行时内容:www 下的设计期图片不入包(与旧版 exclude 行为一致)
rm -f "$STAGE/app/www/"*.png "$STAGE/app/www/"*.jpg
rm -rf "$STAGE/app/www/public" "$STAGE/app/qwenpaw"
rm -rf "${STAGE:?}/"*/__pycache__

# staging 阶段统一执行位:Git 模式之外再设一道(压缩包/特殊文件系统构建时可能丢位)
chmod +x "$STAGE"/app/ui/*.cgi "$STAGE"/cmd/* 2>/dev/null || true
chmod -x "$STAGE/cmd/lib.sh" "$STAGE/cmd/supervisor_ctl.sh" 2>/dev/null || true  # 仅被 source,不需执行位
chmod 644 "$STAGE/manifest" "$STAGE/LICENSE" "$STAGE"/ICON*.PNG 2>/dev/null || true

# 监督者二进制入包(app/bin/,运行时位于 /var/apps/qwenpaw/app/bin/)
mkdir -p "$STAGE/app/bin"
cp "$SUP_BIN_SRC" "$STAGE/app/bin/qwenpaw-supervisor"
chmod +x "$STAGE/app/bin/qwenpaw-supervisor"

# 控制台以编译产物覆盖(app/www/index.html 已迁移为 web/src 源,仓库不存生成物)
if [ ! -f "$PROJ_DIR/web/dist/index.html" ]; then
  echo -e "${RED}ERROR: web/dist/index.html 缺失,控制台编译未执行${NC}"
  exit 1
fi
cp "$PROJ_DIR/web/dist/index.html" "$STAGE/app/www/index.html"

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
"$FNPACK_BIN" build -d . || {
  echo -e "${RED}ERROR: fnpack 构建失败${NC}"
  exit 1
}

FPK_IN_STAGE=$(ls "$STAGE"/*.fpk 2>/dev/null | head -1)
if [ -z "$FPK_IN_STAGE" ] || [ ! -f "$FPK_IN_STAGE" ]; then
  echo -e "${RED}ERROR: fnpack 构建失败，未产出 fpk 文件${NC}"
  exit 1
fi
mv "$FPK_IN_STAGE" "$OUTPUT"

# [3b/4] FPK 内容审计(白名单/执行位/构建残留) —— 失败即出包失败
echo "[3b/4] FPK 内容审计 ..."
bash "$PROJ_DIR/scripts/audit-fpk.sh" "$OUTPUT" || {
  echo -e "${RED}ERROR: FPK 审计未通过,产物已拒绝(见上方审计输出)${NC}"
  rm -f "$OUTPUT"
  exit 1
}

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
