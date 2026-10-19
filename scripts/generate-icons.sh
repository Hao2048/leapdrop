#!/usr/bin/env bash
# ui/brand/icon.png 为唯一图标源文件；仅将桌面应用所需资源写回源码目录。
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"
mkdir -p target
GENERATED="$(mktemp -d "$PROJECT_ROOT/target/icon-assets.XXXXXX")"
trap 'rm -rf "$GENERATED"' EXIT
cargo tauri icon ui/brand/icon.png --output "$GENERATED"
for name in 32x32.png 128x128.png 128x128@2x.png icon.png icon.icns icon.ico; do
  cp "$GENERATED/$name" "src-tauri/icons/$name"
done
