#!/bin/bash
set -e
export PATH=$HOME/tools/llvm-cross/bin:$PATH
B=/Users/penghao/Documents/AIProduct/leapdrop/src-tauri/target/aarch64-apple-ios-sim/release/build
W=$(mktemp -d); cd "$W"

# 需要全局化的符号（仅这些；其余保持局部，避免跨库同名冲突）
TAURI_SYMS="_log_stdout _on_webview_created _register_plugin _run_plugin_command"
SWIFTRS_SYMS="_retain_object _release_object _string_from_bytes"
DIALOG_SYMS="_init_plugin_dialog"

globalize() { # $1=obj $2=syms
  local obj="$1" syms="$2"
  local args=()
  for s in $syms; do args+=(--globalize-symbol="$s"); done
  [ ${#args[@]} -gt 0 ] && llvm-objcopy "${args[@]}" "$obj"
}

rebuild_lib() { # $1=products_dir $2=libname $3="Tauri.o SwiftRs.o ..."  额外交互见下
  local dir="$1" lib="$2"; shift 2
  rm -f "$dir/$lib"
  # 从未被改过的顶层 .o 副本重建
  local tmpd=$(mktemp -d)
  for m in "$@"; do cp "$dir/$m" "$tmpd/"; done
  case "$lib" in
    libTauri.a)
      globalize "$tmpd/Tauri.o" "$TAURI_SYMS"
      globalize "$tmpd/SwiftRs.o" "$SWIFTRS_SYMS" ;;
    libtauri-plugin-dialog.a)
      globalize "$tmpd/tauri-plugin-dialog.o" "$DIALOG_SYMS" ;;
  esac
  (cd "$tmpd" && llvm-ar rcs "$W/out.a" ./*.o && mv "$W/out.a" "$dir/$lib")
  rm -rf "$tmpd"
  echo "rebuilt $dir/$lib"
}

for d in $B/tauri-*/out/swift-rs/Tauri/out/Products/Release-iphonesimulator; do
  rebuild_lib "$d" libTauri.a Tauri.o SwiftRs.o
done
for d in $B/tauri-plugin-dialog-*/out/swift-rs/tauri-plugin-dialog/out/Products/Release-iphonesimulator; do
  rebuild_lib "$d" libtauri-plugin-dialog.a Tauri.o SwiftRs.o tauri-plugin-dialog.o
done
cd /; rm -rf "$W"
echo "=== 验证:"
for A in $(find $B -name "libTauri.a"); do llvm-nm "$A" | grep -w "_log_stdout" | head -1; done
