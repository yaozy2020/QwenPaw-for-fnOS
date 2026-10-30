#!/bin/bash
# sync-upstream.sh — 按 tag 同步上游 QwenPaw 源码到 app/qwenpaw/code/
#
# 用法:
#   bash scripts/sync-upstream.sh              # 同步 .upstream-version 里钉住的版本
#   bash scripts/sync-upstream.sh v2.3.0-beta.1 # 同步指定 tag
#
# 说明:
#   - 上游源码不进 git（仓库只保留工程层），每次换基座跑一次本脚本
#   - 优先用本地镜像 ${PROJ_DIR}/QwenPaw（git clone 的 agentscope-ai/QwenPaw），
#     没有就走 GitHub codeload 拉 tarball
#   - scripts/patches/*.patch 会在同步后自动套用，承载自维护线的本地改动
#
# 注意：不用 set -e（工程铁律），失败路径显式 exit 1

PROJ_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CODE_DIR="${PROJ_DIR}/app/qwenpaw/code"
PATCH_DIR="${PROJ_DIR}/scripts/patches"
VERSION_FILE="${PROJ_DIR}/.upstream-version"
MIRROR_DIR="${PROJ_DIR}/QwenPaw"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'

TAG="${1:-}"
if [ -z "$TAG" ]; then
  if [ ! -f "$VERSION_FILE" ]; then
    echo -e "${RED}ERROR: 未指定 tag 且不存在 $VERSION_FILE${NC}" >&2
    exit 1
  fi
  TAG="$(tr -d '[:space:]' < "$VERSION_FILE")"
fi
if [ -z "$TAG" ]; then
  echo -e "${RED}ERROR: 版本号为空${NC}" >&2
  exit 1
fi

echo "==> 同步上游 QwenPaw ${TAG}"

TMP_DIR="$(mktemp -d)"
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

SRC_DIR=""
if [ -d "$MIRROR_DIR/.git" ]; then
  echo "    使用本地镜像 ${MIRROR_DIR}"
  git -C "$MIRROR_DIR" fetch --tags origin >/dev/null 2>&1 || echo -e "${YELLOW}    警告: fetch 失败，使用本地已有标签${NC}"
  if git -C "$MIRROR_DIR" rev-parse "refs/tags/${TAG}" >/dev/null 2>&1; then
    git clone --branch "$TAG" --depth 1 "file://${MIRROR_DIR}" "$TMP_DIR/src" >/dev/null 2>&1
    SRC_DIR="$TMP_DIR/src"
  else
    echo -e "${YELLOW}    镜像中无标签 ${TAG}，回退 GitHub${NC}"
  fi
fi

if [ -z "$SRC_DIR" ]; then
  echo "    从 GitHub codeload 拉取 ..."
  TARBALL="$TMP_DIR/upstream.tar.gz"
  URL="https://codeload.github.com/agentscope-ai/QwenPaw/tar.gz/refs/tags/${TAG}"
  if ! curl -sL --max-time 600 -o "$TARBALL" "$URL"; then
    echo -e "${RED}ERROR: 下载失败: $URL${NC}" >&2
    exit 1
  fi
  if [ ! -s "$TARBALL" ]; then
    echo -e "${RED}ERROR: 下载内容为空，标签 ${TAG} 可能不存在${NC}" >&2
    exit 1
  fi
  mkdir -p "$TMP_DIR/x"
  tar -xzf "$TARBALL" -C "$TMP_DIR/x" || {
    echo -e "${RED}ERROR: 解压失败${NC}" >&2
    exit 1
  }
  SRC_DIR="$(find "$TMP_DIR/x" -maxdepth 1 -mindepth 1 -type d | head -1)"
  if [ -z "$SRC_DIR" ]; then
    echo -e "${RED}ERROR: 解压后未找到源码目录${NC}" >&2
    exit 1
  fi
fi

echo "    上游内容: $(du -sh "$SRC_DIR" | cut -f1)"

# 保留本地构建产物不被 --delete 抹掉
PRESERVE=""
[ -d "$CODE_DIR/src/qwenpaw/console" ] && PRESERVE="$PRESERVE --exclude=src/qwenpaw/console"

echo "==> 覆盖 ${CODE_DIR}"
mkdir -p "$CODE_DIR"
rsync -a --delete $PRESERVE "$SRC_DIR"/ "$CODE_DIR"/ || {
  echo -e "${RED}ERROR: rsync 失败（rsync 是否可用？）${NC}" >&2
  exit 1
}

# 套用自维护补丁
if [ -d "$PATCH_DIR" ] && ls "$PATCH_DIR"/*.patch >/dev/null 2>&1; then
  echo "==> 套用本地补丁"
  for p in "$PATCH_DIR"/*.patch; do
    echo "    $(basename "$p")"
    if ! (cd "$CODE_DIR" && patch -p1 --forward --batch < "$p"); then
      echo -e "${YELLOW}    警告: 补丁 $(basename "$p") 未完全应用，请人工确认${NC}"
    fi
  done
fi

echo "$TAG" > "$VERSION_FILE"
echo -e "${GREEN}==> 完成，上游基座 = ${TAG}${NC}"
echo "    （补丁目录: ${PATCH_DIR}，当前补丁数: $(ls "$PATCH_DIR"/*.patch 2>/dev/null | wc -l)）"
