#!/usr/bin/env bash
# 把 help/diagrams/*.svg 渲染成 help/images/*.png
#
# 帮助中心的图片是运行时从 help/ 目录读的（见 src/help/render.rs 的 ImageCache），
# 不参与编译；DMG 由 scripts/make-dmg.sh 把 help/images 整个拷进
# Contents/Resources/help/images。所以这里只需要产出 PNG 文件即可。
#
# 为什么用无头浏览器：仓库不引入 SVG 光栅化依赖，macOS/CI 上也不保证有
# rsvg-convert 或 inkscape。Edge / Chrome 的 headless 截图在各平台都能拿到，
# 且能正确渲染中文字体。
#
# 用法：
#   scripts/make-help-diagrams.sh            # 全量重渲染
#   scripts/make-help-diagrams.sh strategy   # 只渲染文件名含 strategy 的
#
# 输出统一 2 倍图（Retina）：SVG 里的 width/height 是逻辑尺寸，PNG 是其两倍像素。

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/help/diagrams"
OUT="$ROOT/help/images"
FILTER="${1:-}"

if [[ ! -d "$SRC" ]]; then
    echo "找不到图表源目录: $SRC" >&2
    exit 1
fi
mkdir -p "$OUT"

BROWSER="${MNS_HELP_BROWSER:-}"
if [[ -z "$BROWSER" ]]; then
    for candidate in \
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" \
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
        "/Applications/Chromium.app/Contents/MacOS/Chromium" \
        "$(command -v google-chrome || true)" \
        "$(command -v chromium || true)"
    do
        if [[ -n "$candidate" && -x "$candidate" ]]; then
            BROWSER="$candidate"
            break
        fi
    done
fi
if [[ -z "$BROWSER" ]]; then
    echo "找不到可用的无头浏览器；可用 MNS_HELP_BROWSER=/path/to/chrome 指定" >&2
    exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

render_one() {
    local svg="$1"
    local name width height html png
    name="$(basename "$svg" .svg)"
    # 尺寸写在 <svg width=".." height=".."> 上；缺失就报错，不猜
    width="$(sed -n 's/.*<svg[^>]* width="\([0-9][0-9]*\)".*/\1/p' "$svg" | head -1)"
    height="$(sed -n 's/.*<svg[^>]* height="\([0-9][0-9]*\)".*/\1/p' "$svg" | head -1)"
    if [[ -z "$width" || -z "$height" ]]; then
        echo "  ✗ $name: <svg> 缺少整数 width/height 属性" >&2
        return 1
    fi

    # 包一层 HTML：直接截图 SVG 文件会带上浏览器默认的页面边距
    html="$TMP/$name.html"
    {
        printf '<!doctype html><meta charset="utf-8">'
        printf '<style>html,body{margin:0;padding:0;background:transparent}svg{display:block}</style>'
        cat "$svg"
    } > "$html"

    png="$OUT/$name.png"
    "$BROWSER" --headless=new --no-sandbox --disable-gpu --disable-dev-shm-usage \
        --hide-scrollbars --force-device-scale-factor=2 \
        --window-size="$width,$height" \
        --screenshot="$png" "file://$html" >/dev/null 2>&1

    if [[ ! -f "$png" ]]; then
        echo "  ✗ $name: 渲染失败" >&2
        return 1
    fi
    printf '  ✓ %-32s %sx%s → %s\n' "$name" "$width" "$height" "$(du -h "$png" | cut -f1)"
}

count=0
for svg in "$SRC"/*.svg; do
    [[ -e "$svg" ]] || continue
    if [[ -n "$FILTER" && "$(basename "$svg")" != *"$FILTER"* ]]; then
        continue
    fi
    render_one "$svg"
    count=$((count + 1))
done

echo "完成：$count 张，输出到 ${OUT#"$ROOT"/}"
