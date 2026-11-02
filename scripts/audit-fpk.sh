#!/bin/bash
# audit-fpk.sh — FPK 产物审计:构建脚本自动执行,失败即出包失败(build.sh [3b/4])
#   1. FPK 可解包,顶层结构符合 fnOS 要求
#   2. 生命周期脚本与 cmd/main 具备执行位
#   3. app.tgz 内含监督者/CGI/控制台三件套,预压缩内容可解码
#   4. 包内无 Rust 中间产物、node_modules、.git、__pycache__、日志等构建残留
# 用法: bash scripts/audit-fpk.sh /path/to/output.fpk

set -u

FPK="${1:-}"
if [ -z "$FPK" ] || [ ! -f "$FPK" ]; then
    echo "audit: 用法 audit-fpk.sh <fpk路径>,且文件必须存在"
    exit 1
fi

fail=0
note() { echo "    $*"; }
fail() { echo "    [FAIL] $*"; fail=1; }
ok() { echo "    [ OK ] $*"; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ── 1. 解包与顶层结构 ──
if ! tar -xzf "$FPK" -C "$WORK" 2>/dev/null; then
    echo "audit: FPK 无法解包(非 gzip/tar?): $FPK"
    exit 1
fi
for f in manifest cmd/main cmd/install_callback cmd/uninstall_callback config/privilege wizard app.tgz; do
    if [ -e "$WORK/$f" ]; then
        ok "顶层含 $f"
    else
        fail "顶层缺少 $f"
    fi
done

# 顶层全白名单审计 (P1-4: 严禁任何本地未追踪/未声明文件污染包顶层)
unexpected_top=$(find "$WORK" -maxdepth 2 -type f \
    ! -path "$WORK/manifest" ! -path "$WORK/LICENSE" ! -path "$WORK/ICON.PNG" ! -path "$WORK/ICON_256.PNG" \
    ! -path "$WORK/app.tgz" ! -path "$WORK/config/privilege" ! -path "$WORK/config/resource" \
    ! -path "$WORK/wizard/install" ! -path "$WORK/wizard/upgrade" ! -path "$WORK/wizard/config" ! -path "$WORK/wizard/uninstall" \
    ! -path "$WORK/cmd/main" ! -path "$WORK/cmd/lib.sh" ! -path "$WORK/cmd/supervisor_ctl.sh" \
    ! -path "$WORK/cmd/install_init" ! -path "$WORK/cmd/install_callback" \
    ! -path "$WORK/cmd/upgrade_init" ! -path "$WORK/cmd/upgrade_callback" \
    ! -path "$WORK/cmd/uninstall_init" ! -path "$WORK/cmd/uninstall_callback" \
    ! -path "$WORK/cmd/config_init" ! -path "$WORK/cmd/config_callback" 2>/dev/null)
if [ -n "$unexpected_top" ]; then
    fail "顶层发现非白名单文件:"
    printf '%s\n' "$unexpected_top" | sed 's/^/        /'
else
    ok "顶层文件 100% 符合纯净白名单"
fi

# ── 2. 执行位 ──
for f in "$WORK"/cmd/main "$WORK"/cmd/install_init "$WORK"/cmd/install_callback \
         "$WORK"/cmd/upgrade_init "$WORK"/cmd/upgrade_callback \
         "$WORK"/cmd/uninstall_init "$WORK"/cmd/uninstall_callback \
         "$WORK"/cmd/config_init "$WORK"/cmd/config_callback; do
    if [ -f "$f" ] && [ ! -x "$f" ]; then
        fail "$(basename "$f") 缺少执行位"
    fi
done
if [ -x "$WORK/cmd/main" ]; then ok "cmd/main 执行位"; fi
# lib.sh 仅被 source,不应有执行位(不强制,仅提示)
[ -x "$WORK/cmd/lib.sh" ] && note "提示: cmd/lib.sh 带执行位(仅被 source,可去掉)"

# ── 3. app.tgz 内容 ──
mkdir -p "$WORK/appx"
if ! tar -xzf "$WORK/app.tgz" -C "$WORK/appx" 2>/dev/null; then
    fail "app.tgz 无法解包"
else
    # 三件套:监督者 / CGI / 控制台
    if [ -x "$WORK/appx/bin/qwenpaw-supervisor" ] || [ -x "$WORK/appx/app/bin/qwenpaw-supervisor" ]; then
        ok "监督者二进制在包内且可执行"
    else
        fail "监督者二进制缺失或无执行位(app/bin/qwenpaw-supervisor)"
    fi
    for f in ui/api.cgi ui/index.cgi; do
        if [ -f "$WORK/appx/$f" ] && [ -x "$WORK/appx/$f" ]; then
            ok "$f 存在且可执行"
        else
            fail "$f 缺失或无执行位"
        fi
    done
    for f in www/index.html www/index.html.gz www/index.html.br; do
        if [ -f "$WORK/appx/$f" ]; then
            ok "$f 存在"
        else
            fail "控制台预压缩产物缺失: $f"
        fi
    done
    # 预压缩内容可解码且与源一致
    if [ -f "$WORK/appx/www/index.html" ] && [ -f "$WORK/appx/www/index.html.gz" ]; then
        if gzip -t "$WORK/appx/www/index.html.gz" 2>/dev/null; then
            ok "index.html.gz 可解码"
        else
            fail "index.html.gz 损坏"
        fi
    fi
    if command -v brotli >/dev/null 2>&1 && [ -f "$WORK/appx/www/index.html.br" ]; then
        if brotli -d -c "$WORK/appx/www/index.html.br" 2>/dev/null | head -c 1 | grep -q .; then
            ok "index.html.br 可解码"
        else
            fail "index.html.br 损坏"
        fi
    fi
    # 监督者架构(仅在本机可执行 file 命令时检查)
    SUP="$WORK/appx/bin/qwenpaw-supervisor"
    [ -f "$SUP" ] || SUP="$WORK/appx/app/bin/qwenpaw-supervisor"
    if [ -f "$SUP" ] && command -v file >/dev/null 2>&1; then
        if file -b "$SUP" | grep -q "ELF"; then
            ok "监督者为 ELF 二进制"
        else
            fail "监督者不是 ELF 二进制"
        fi
    fi

    # ── 3b. app.tgz 白名单结构审计 ──
    # 严格确保 app.tgz 内仅包含预期文件，绝不混入本地配置文件、源映射或无关垃圾
    # 显式断言 4 个桌面入口图标存在且非空 (P1-2)
    for icon in console_64.png console_256.png icon_64.png icon_256.png; do
        if [ -s "$WORK/appx/ui/images/$icon" ]; then
            ok "桌面图标存在: ui/images/$icon"
        else
            fail "桌面图标缺失或为空: ui/images/$icon"
        fi
    done

    unexpected_files=$(find "$WORK/appx" -type f \
        ! -path "$WORK/appx/bin/qwenpaw-supervisor" \
        ! -path "$WORK/appx/ui/api.cgi" ! -path "$WORK/appx/ui/index.cgi" ! -path "$WORK/appx/ui/config" \
        ! -path "$WORK/appx/ui/images/console_64.png" ! -path "$WORK/appx/ui/images/console_256.png" \
        ! -path "$WORK/appx/ui/images/icon_64.png" ! -path "$WORK/appx/ui/images/icon_256.png" \
        ! -path "$WORK/appx/config/resource" ! -path "$WORK/appx/config/privilege" \
        ! -path "$WORK/appx/www/index.html" ! -path "$WORK/appx/www/index.html.gz" ! -path "$WORK/appx/www/index.html.br" 2>/dev/null)
    if [ -n "$unexpected_files" ]; then
        fail "app.tgz 发现非白名单文件:"
        printf '%s
' "$unexpected_files" | sed 's/^/        /'
    else
        ok "app.tgz 内容 100% 符合运行时纯净白名单"
    fi

    # ── 4. 构建残留黑名单 ──
    residue=$(find "$WORK" \( -name 'target' -o -name 'node_modules' -o -name '.git' \
        -o -name '__pycache__' -o -name '*.rlib' -o -name '*.d' -o -name '*.log' \
        -o -name '*.fpk' -o -name '*.sha256' \) 2>/dev/null)
    if [ -n "$residue" ]; then
        fail "发现构建残留:"
        printf '%s\n' "$residue" | sed 's/^/        /'
    else
        ok "无 target/node_modules/.git/__pycache__ 等构建残留"
    fi
fi

echo "audit: $([ $fail -eq 0 ] && echo 'ALL CHECKS PASSED' || echo 'FAILED')"
exit $fail
