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
FPK_OUT="${FPK_OUT:-${PROJ_DIR}/build/output}"
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

# [1b/4] 构建监督者(release 单二进制; Release 必须干净编译)
SUP_BIN_SRC="$PROJ_DIR/supervisor/target/release/qwenpaw-supervisor"
if command -v cargo >/dev/null 2>&1 || [ -x "$HOME/.cargo/bin/cargo" ]; then
  echo "[1b/4] 构建监督者 (cargo release --locked) ..."
  rm -f "$SUP_BIN_SRC"
  (cd "$PROJ_DIR/supervisor" && cargo build --locked --release) || {
    echo -e "${RED}ERROR: 监督者 cargo 构建失败,终止出包${NC}"
    exit 1
  }
elif [ "${ALLOW_STALE_BINARY:-0}" = "1" ] && [ -x "$SUP_BIN_SRC" ]; then
  echo -e "${YELLOW}[1b/4] 警告: 启用 ALLOW_STALE_BINARY=1,复用已有监督者产物(非全新构建)${NC}"
else
  echo -e "${RED}ERROR: 未找到 cargo 工具链。发布构建必须通过 cargo 编译最新产物。${NC}"
  echo -e "${RED}       若在本地调试且确需复用已有二进制,请显式设置 ALLOW_STALE_BINARY=1${NC}"
  exit 1
fi
if [ ! -x "$SUP_BIN_SRC" ]; then
  echo -e "${RED}ERROR: 监督者二进制缺失或不可执行($SUP_BIN_SRC)${NC}"
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
GIT_SHA="$(git -C "$PROJ_DIR" rev-parse --short HEAD 2>/dev/null || echo "${SOURCE_COMMIT:-unknown}")"
BUILD_ID="v${VERSION}+${GIT_SHA}"
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
cp -a "$PROJ_DIR/cmd" "$STAGE/cmd"
cp -a "$PROJ_DIR/config" "$STAGE/config"
cp -a "$PROJ_DIR/wizard" "$STAGE/wizard"

# app 目录严禁整目录浅拷贝，严格按运行时白名单物化:
mkdir -p "$STAGE/app/ui" "$STAGE/app/www" "$STAGE/app/bin"
cp "$PROJ_DIR/app/ui/api.cgi" "$STAGE/app/ui/api.cgi"
cp "$PROJ_DIR/app/ui/index.cgi" "$STAGE/app/ui/index.cgi"
cp "$PROJ_DIR/app/ui/config" "$STAGE/app/ui/config"

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

# [3/4] 调用 fpkstudio 引擎打包 (https://github.com/shuangji66/fpkstudio)
echo "[3/4] fpkstudio 极速构建打包 (Unix Socket 引擎) ..."
export FPK_OUT
python3 "$PROJ_DIR/tools/fpkstudio-pack.py" || {
  echo -e "${RED}ERROR: fpkstudio 构建打包失败${NC}"
  exit 1
}

OUTPUT="$(ls -t "${FPK_OUT}"/qwenpaw*.fpk 2>/dev/null | head -1)"
if [ -z "$OUTPUT" ] || [ ! -f "$OUTPUT" ]; then
  echo -e "${RED}ERROR: 未找到产出的 fpk 文件${NC}"
  exit 1
fi
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
