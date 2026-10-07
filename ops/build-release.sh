#!/usr/bin/env bash
# YukiPan release 构建（开发机用）：在 yukipan/ workspace 下 cargo build --release，
# 收集后端二进制（crate 二进制名 yukipan，打包为 bin/yukipan-server 以对齐部署约定）；
# 前端未定档：存在 web/dist 时一并打进 web/，否则只告警。
# 用法：./ops/build-release.sh <YYYYMMDDTHHMMSSZ-gitsha> [输出目录]
# 产物：<输出目录>/yukipan-<version>.tar.gz 与同名 .sha256（rehearsal 期望的路径）。

set -Eeuo pipefail
umask 022

readonly OPS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly ROOT="$(cd -- "$OPS_DIR/.." && pwd)"
readonly BACKEND_ROOT="$ROOT/yukipan"
readonly VERSION="${1:-}"
readonly OUTPUT_DIR="${2:-$ROOT/.release-build/output}"
readonly BUILD_ROOT="${YUKIPAN_BUILD_ROOT:-$ROOT/.release-build}"
readonly STAGE="$BUILD_ROOT/stage"
# 默认复用 yukipan/ 下已有的 target 与系统 Cargo 缓存，加速半成品代码的反复构建
readonly TARGET_DIR="${CARGO_TARGET_DIR:-$BACKEND_ROOT/target}"
readonly TMP_DIR="${TMPDIR:-$BUILD_ROOT/tmp}"
readonly WEB_DIST="${YUKIPAN_WEB_DIST:-$ROOT/web/dist}"

if [[ ! "$VERSION" =~ ^[0-9]{8}T[0-9]{6}Z-[a-f0-9]{7,40}$ ]]; then
    printf '用法：%s <YYYYMMDDTHHMMSSZ-gitsha> [输出目录]\n' "$0" >&2
    exit 2
fi

[[ -f "$BACKEND_ROOT/Cargo.toml" ]] || { echo "缺少后端 workspace：$BACKEND_ROOT" >&2; exit 1; }
command -v cargo >/dev/null || { echo "缺少 cargo" >&2; exit 1; }
command -v sha256sum >/dev/null || { echo "缺少 sha256sum" >&2; exit 1; }

rm -rf -- "$STAGE"
mkdir -p -- "$STAGE/bin" "$OUTPUT_DIR" "$TMP_DIR"

(
    cd -- "$BACKEND_ROOT"
    CARGO_TARGET_DIR="$TARGET_DIR" \
    TMPDIR="$TMP_DIR" \
    CARGO_INCREMENTAL=0 \
        cargo build --release
)

[[ -f "$TARGET_DIR/release/yukipan" ]] \
    || { echo "缺少构建产物：$TARGET_DIR/release/yukipan（后端可能尚未可编译）" >&2; exit 1; }
install -m 755 "$TARGET_DIR/release/yukipan" "$STAGE/bin/yukipan-server"

# 前端未定档：有 dist 就一起打包，没有就只发后端
manifest_dirs=(bin)
if [[ -d "$WEB_DIST" ]]; then
    mkdir -p -- "$STAGE/web"
    cp -a -- "$WEB_DIST/." "$STAGE/web/"
    manifest_dirs+=(web)
else
    echo "警告：未找到前端产物 $WEB_DIST，release 仅含后端" >&2
fi
printf '%s\n' "$VERSION" > "$STAGE/RELEASE"

(
    cd -- "$STAGE"
    find "${manifest_dirs[@]}" -type f -print0 \
        | LC_ALL=C sort -z \
        | xargs -0 sha256sum > MANIFEST.sha256
    sha256sum RELEASE >> MANIFEST.sha256
)

readonly ARCHIVE="$OUTPUT_DIR/yukipan-$VERSION.tar.gz"
tar -C "$STAGE" -czf "$ARCHIVE" .
(
    cd "$OUTPUT_DIR"
    sha256sum "$(basename "$ARCHIVE")" > "$(basename "$ARCHIVE").sha256"
)
printf 'Release: %s\nSHA-256: %s\n' \
    "$ARCHIVE" "$(cut -d' ' -f1 "$ARCHIVE.sha256")"
