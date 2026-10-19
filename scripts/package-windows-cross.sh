#!/usr/bin/env bash
# 在 macOS/Linux 上交叉编译 Windows x64；需 cargo-xwin、LLVM 和 Windows Rust target。
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"
if [ -n "${LLVM_BIN:-}" ]; then export PATH="$LLVM_BIN:$PATH"; fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target/windows-build}"
cargo xwin build -p leapdrop --release --target x86_64-pc-windows-msvc --locked
python3 - "$PROJECT_ROOT" "$CARGO_TARGET_DIR" <<'PY'
import json, sys, zipfile
from pathlib import Path
root, build = map(Path, sys.argv[1:])
version = json.loads((root / 'src-tauri/tauri.conf.json').read_text())['version']
output = root / 'target/release-artifacts'
output.mkdir(parents=True, exist_ok=True)
archive = output / f'LeapDrop_{version}_Windows_x64.zip'
with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    z.write(build / 'x86_64-pc-windows-msvc/release/leapdrop.exe', 'leapdrop.exe')
    z.write(root / 'docs/quick-start.txt', '使用说明.txt')
    z.write(root / 'LICENSE', 'LICENSE')
with zipfile.ZipFile(archive) as z:
    assert z.testzip() is None, 'ZIP validation failed'
print(archive)
PY
