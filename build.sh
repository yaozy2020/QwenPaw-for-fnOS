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
# 版本号提取必须有失败保护:manifest 格式漂移(缩进/换行/键改名)会让这里静默得到
# 空串,产出文件名变成 "qwenpaw--x86.fpk" 并一路带到 Release 里。
if [ -z "$VERSION" ]; then
  echo "ERROR: 无法从 manifest 提取 version(检查 '^version' 行格式)" >&2
  exit 1
fi
if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9]+)?$'; then
  echo "ERROR: manifest version '$VERSION' 不符合 X.Y.Z[-N] 格式" >&2
  exit 1
fi
# 输出路径可被环境变量覆盖（CI 用 FPK_OUT=$GITHUB_WORKSPACE）；
# 默认是 build/output 目录。
FPK_OUT="${FPK_OUT:-${PROJ_DIR}/build/output}"
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$PROJ_DIR" log -1 --format=%ct 2>/dev/null || echo 0)}"
export SOURCE_DATE_EPOCH
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
# 探测与调用必须用同一路径:探测接受 ~/.cargo/bin 而调用裸 cargo,
# PATH 不含它的场景下会"有 cargo"却 command not found,错误归因失真
CARGO_BIN="$(command -v cargo 2>/dev/null || true)"
[ -z "$CARGO_BIN" ] && [ -x "$HOME/.cargo/bin/cargo" ] && CARGO_BIN="$HOME/.cargo/bin/cargo"
if [ -n "$CARGO_BIN" ]; then
  echo "[1b/4] 构建监督者 (cargo release --locked) ..."
  rm -f "$SUP_BIN_SRC"
  (cd "$PROJ_DIR/supervisor" && "$CARGO_BIN" build --locked --release) || {
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
    npm ci --registry=https://registry.npmmirror.com --no-audit --no-fund 2>&1 | tail -20) || {
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

# [2/4] staging 唯一权威白名单目录树与权限归一化
echo "[2/4] staging 唯一权威白名单物化与权限归一化 ..."
rm -rf "$STAGE"
mkdir -p "$STAGE"

# 顶层纯净白名单:清单/许可/图标 (mode 0644)
cp "$PROJ_DIR/manifest" "$STAGE/manifest"
cp "$PROJ_DIR/LICENSE" "$STAGE/LICENSE" || { echo "ERROR: 复制 LICENSE 失败"; exit 1; }
cp "$PROJ_DIR/ICON.PNG" "$PROJ_DIR/ICON_256.PNG" "$STAGE/" || { echo "ERROR: 复制图标失败"; exit 1; }
chmod 0644 "$STAGE/manifest" "$STAGE/LICENSE" "$STAGE/ICON.PNG" "$STAGE/ICON_256.PNG"

# cmd 目录白名单精准物化 (严禁任何未跟踪的测试/.pid残留入包)
mkdir -p "$STAGE/cmd"
for f in main install_init install_callback upgrade_init upgrade_callback uninstall_init uninstall_callback config_init config_callback; do
  cp "$PROJ_DIR/cmd/$f" "$STAGE/cmd/$f"
  chmod 0755 "$STAGE/cmd/$f"
done
cp "$SUP_BIN_SRC" "$STAGE/cmd/qwenpaw-supervisor"
chmod 0755 "$STAGE/cmd/qwenpaw-supervisor"

# config 目录白名单精准物化 (mode 0644)
mkdir -p "$STAGE/config"
cp "$PROJ_DIR/config/privilege" "$STAGE/config/privilege"
cp "$PROJ_DIR/config/resource" "$STAGE/config/resource" || { echo "ERROR: 复制 config/resource 失败"; exit 1; }
chmod 0644 "$STAGE/config/privilege" "$STAGE/config/resource"

# wizard 目录白名单精准物化 (mode 0644)
mkdir -p "$STAGE/wizard"
for f in install upgrade config uninstall; do
  [ -f "$PROJ_DIR/wizard/$f" ] && cp "$PROJ_DIR/wizard/$f" "$STAGE/wizard/$f" && chmod 0644 "$STAGE/wizard/$f"
done

# app 运行时白名单精准物化 (统一 Gateway 静态控制台与 4 个桌面图标)
mkdir -p "$STAGE/app/ui/images" "$STAGE/app/www" "$STAGE/app/bin"

cp "$PROJ_DIR/app/ui/config" "$STAGE/app/ui/config"
chmod 0644 "$STAGE/app/ui/config"

# 复制 4 个桌面入口图标 (P1-2 修复)
for icon in console_64.png console_256.png icon_64.png icon_256.png; do
  if [ -f "$PROJ_DIR/app/ui/images/$icon" ]; then
    cp "$PROJ_DIR/app/ui/images/$icon" "$STAGE/app/ui/images/$icon"
    chmod 0644 "$STAGE/app/ui/images/$icon"
  else
    echo -e "${RED}ERROR: 必需桌面图标缺失: app/ui/images/$icon${NC}"
    exit 1
  fi
done

# 监督者二进制入包 (mode 0755)
cp "$SUP_BIN_SRC" "$STAGE/app/bin/qwenpaw-supervisor"
chmod 0755 "$STAGE/app/bin/qwenpaw-supervisor"

# 控制台以编译产物覆盖并生成预压缩兄弟文件
if [ ! -f "$PROJ_DIR/web/dist/index.html" ]; then
  echo -e "${RED}ERROR: web/dist/index.html 缺失,控制台编译未执行${NC}"
  exit 1
fi
CONSOLE_HTML="$STAGE/app/www/index.html"
cp "$PROJ_DIR/web/dist/index.html" "$CONSOLE_HTML"
chmod 0644 "$CONSOLE_HTML"

if [ -n "$NODE_BIN" ] && [ -x "$NODE_BIN" ]; then
  "$NODE_BIN" -e 'const {readFileSync,writeFileSync}=require("fs"),{gzipSync,brotliCompressSync,constants}=require("zlib");const f=process.argv[1],b=readFileSync(f);writeFileSync(f+".gz",gzipSync(b,{level:9}));writeFileSync(f+".br",brotliCompressSync(b,{params:{[constants.BROTLI_PARAM_QUALITY]:11}}))' "$CONSOLE_HTML" \
    && chmod 0644 "$CONSOLE_HTML.gz" "$CONSOLE_HTML.br" \
    && echo "    预压缩: index.html.gz/.br 已生成" \
    || echo -e "${YELLOW}    警告: 预压缩生成失败，运行时回退未压缩${NC}"
elif command -v gzip >/dev/null 2>&1; then
  gzip -9 -c "$CONSOLE_HTML" > "$CONSOLE_HTML.gz" && chmod 0644 "$CONSOLE_HTML.gz" && echo "    预压缩: 无 node，仅生成 index.html.gz"
fi

# 统一所有目录为 0755 (P1-3 权限归一化)
find "$STAGE" -type d -exec chmod 0755 {} +

# [3/4] 调用 fpkstudio 引擎基于 staging 封装 (单一真实物料链)
echo "[3/4] fpkstudio 极速构建打包 (Unix Socket 引擎) ..."
export FPK_OUT
PACK_OUT="$(python3 "$PROJ_DIR/tools/fpkstudio-pack.py" "$STAGE")" || {
  echo -e "${RED}ERROR: fpkstudio 构建打包失败${NC}"
  exit 1
}
echo "$PACK_OUT"

OUTPUT="$(echo "$PACK_OUT" | grep '^FPK_OUTPUT_PATH=' | cut -d'=' -f2-)"
if [ -z "$OUTPUT" ] || [ ! -f "$OUTPUT" ]; then
  echo -e "${RED}ERROR: 未找到产出的 fpk 文件${NC}"
  exit 1
fi

# [3b/4] FPK 内容审计(白名单/执行位/构建残留) —— 失败即出包失败
echo "[3b/4] FPK 内容审计 ..."
bash "$PROJ_DIR/scripts/audit-fpk.sh" "$OUTPUT" || {
  echo -e "${RED}ERROR: FPK 审计未通过,产物已拒绝(见上方审计输出)${NC}"
  # 陈旧的 .sha256 一并清掉:留下会指向已被拒绝删除的产物,误导下游校验
  rm -f "$OUTPUT" "$OUTPUT.sha256"
  exit 1
}

# [4/4] 计算 SHA256 与归档
echo "[4/4] 计算签名与收尾 ..."
# 以纯文件名写入 .sha256:sha256sum 会把传入路径原样写进输出,
# 用绝对路径会让用户下载后 `sha256sum -c` 因该路径不存在而失败。
( cd "$(dirname "$OUTPUT")" && sha256sum "$(basename "$OUTPUT")" ) > "${OUTPUT}.sha256"
rm -rf "$STAGE"

echo ""
echo -e "${GREEN}===== 纯净 Launcher 打包完成 =====${NC}"
echo "输出: $OUTPUT"
ls -lh "$OUTPUT"
echo ""
echo "SHA256:"
cat "${OUTPUT}.sha256"
