#!/usr/bin/env bash
# 在 Mac 上构建通用版本并生成已 ad-hoc 签名的 DMG。SDKROOT 可由调用方指定。
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"
VERSION="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
ARTIFACT_DIR="$PROJECT_ROOT/target/release-artifacts"
mkdir -p "$ARTIFACT_DIR"
rustup target add aarch64-apple-darwin x86_64-apple-darwin
(cd src-tauri && cargo tauri build --target universal-apple-darwin --bundles app --no-sign --ci -- --locked)
APP="$CARGO_TARGET_DIR/universal-apple-darwin/release/bundle/macos/LeapDrop.app"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
STAGE="$(mktemp -d "$PROJECT_ROOT/target/dmg-stage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/LeapDrop.app"
cp docs/quick-start.txt "$STAGE/使用说明.txt"
cp LICENSE "$STAGE/LICENSE"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname LeapDrop -srcfolder "$STAGE" -ov -format UDZO "$ARTIFACT_DIR/LeapDrop_${VERSION}_macOS_universal.dmg"
hdiutil verify "$ARTIFACT_DIR/LeapDrop_${VERSION}_macOS_universal.dmg"
