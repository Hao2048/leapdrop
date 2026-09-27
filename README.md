# 跃传 LeapDrop

[![release](https://github.com/peng411528/leapdrop/actions/workflows/release.yml/badge.svg)](../../actions)

跨平台局域网文件互传（macOS / Windows / Android）。**设备配对 + mDNS 自动发现 + TLS 直传 + 接收确认 + 断点续传**，无需服务器、文件不出局域网。

## 设备配对（v0.2.0 新增）

每台设备首次启动生成永不变更的 **设备 ID**（形如 `3F7A-92C1`，可在设置页查看）。两台设备**首次互传前需配对一次**：发起方点发送后，对方屏幕弹出配对确认（显示双方设备 ID），双方确认后互相保存，之后免确认直传（每次传文件仍有接收确认）。解除配对：设置 → 已配对设备 → 解除配对。所有传输请求带 Ed25519 设备签名，未配对设备的请求一律拒绝。

## 下载

到 [Releases](../../releases) 页下载对应平台可执行文件：

| 平台 | 文件 | 说明 |
|---|---|---|
| macOS (Apple Silicon / Intel 通用) | `LeapDrop_x.x.x_universal.dmg` | 未签名，首次打开见下方步骤；并允许「本地网络」权限 |
| Android 8+ (arm64) | `leapdrop-android-arm64.apk` | 直接安装；首次运行允许「位置/附近设备」与网络权限 |
| Windows 10/11 x64 | `leapdrop-windows-x64.zip` | 解压运行 `leapdrop.exe`；需系统自带 WebView2 运行时（Win11 已内置）；SmartScreen 提示时点「仍要运行」，防火墙勾选「专用网络」 |

### macOS 首次打开（未签名应用的放行步骤）

双击被系统拦截（「无法验证开发者」/「无法检查是否包含恶意软件」）后，**二选一**：

- **系统设置法**：系统设置 → 隐私与安全性 → 「安全性」区块出现「已阻止使用 LeapDrop」→ 点 **仍要打开** → 输密码 → 再双击即可，之后不会再弹
- **终端法**（把 app 拖进「应用程序」后执行一行）：

  ```bash
  xattr -dr com.apple.quarantine /Applications/LeapDrop.app
  ```

> 注：macOS 13 及更早版本可直接 右键 → 打开；macOS 15+ 不再支持右键绕过，请用上面两种方法。

首次运行还需要允许：macOS 的「本地网络」权限与防火墙；Windows 防火墙（勾选专用网络）。

## 功能（P0）

- 同一局域网内自动发现设备（mDNS / `_leapdrop._tcp.local.`），雷达式设备列表
- 拖拽 / 文件对话框发送，接收方弹窗确认后才开始传输
- 全程 TLS：自签证书 + **证书指纹钉扎**（mDNS TXT 公布指纹，发送方校验对端证书一致，防局域网中间人）
- 4MB 分块并行度友好传输、sha256 整文件校验、断点续传（bitmap 边车）
- 接收文件保存到 `~/Downloads/LeapDrop`，重名自动加序号

## 构建与运行

前置：Rust（rustup stable）、macOS 需要 Xcode Command Line Tools；Windows 需要 VS Build Tools；Android 需要 JDK17 + Android SDK/NDK（`cargo tauri android init` 一次，`cargo tauri android build --apk --target aarch64` 出包）。

```bash
cd src-tauri
cargo run        # 开发运行（首次编译约几分钟）
cargo test       # 集成测试：TLS 回环传输 + 拒绝流程
cargo build --release   # 发布构建（产物在 target/release/leapdrop）
```

### 发布 macOS dmg（关键：打包后必须 ad-hoc 签名）

`cargo tauri build` 产出的 .app **没有完整签名**，直接分发在新版 macOS 上会报「已损坏」且无法放行。必须补一步签名再重新做 dmg：

```bash
cargo tauri build --target universal-apple-darwin
APP=target/universal-apple-darwin/release/bundle/macos/LeapDrop.app
codesign --force --deep --sign - "$APP"     # ad-hoc 完整签名
codesign --verify --deep "$APP"             # 必须输出通过
# 从签名后的 app 重新生成 dmg
rm -rf /tmp/dmgstage && mkdir /tmp/dmgstage
cp -R "$APP" /tmp/dmgstage/ && ln -s /Applications /tmp/dmgstage/Applications
hdiutil create -volname LeapDrop -srcfolder /tmp/dmgstage -ov -format UDZO LeapDrop_0.1.1_universal.dmg
```

签名后收件人首次打开：双击被拦 → 系统设置 → 隐私与安全性 → 「仍要打开」（或 `xattr -dr com.apple.quarantine /Applications/LeapDrop.app`）。若购买 Apple Developer 账号，可将 `-` 换成 Developer ID 证书并做公证，收件人双击直开。

### 发布 Windows exe（macOS 交叉编译）

```bash
# 前置：安装 LLVM（需要 clang-cl/llvm-lib/llvm-rc）并加入 PATH；rustup target add x86_64-pc-windows-msvc
cargo xwin build --release --target x86_64-pc-windows-msvc
# target/x86_64-pc-windows-msvc/release/leapdrop.exe 打 zip 分发
```

> 本机若遇到 `tapi error: malformed file ... MacOSX27.0.sdk`（CLT 新 SDK 与链接器不兼容），仓库已在 `src-tauri/.cargo/config.toml` 固定 `SDKROOT=MacOSX26.5.sdk`。

首次运行 macOS 会弹两个系统授权，都需要允许：

1. **本地网络**（Local Network）——mDNS 发现必需；拒绝后可在 系统设置 → 隐私与安全性 → 本地网络 里重新开启
2. **防火墙**（未签名开发版）——接收连接

Windows 首次运行防火墙弹窗请勾选“专用网络”允许。跨机互传时两台机器都要运行本程序、同一网段（注意 2.4G/5G 频段隔离和 AP 隔离）。

## 目录结构

```
leapdrop/
├── ui/                    # 前端（零构建，纯 HTML/JS，Tauri webview 直接加载）
│   ├── index.html
│   ├── style.css
│   └── app.js
└── src-tauri/
    ├── src/
    │   ├── lib.rs         # Tauri 命令/事件接线、AppState
    │   ├── identity.rs    # 自签证书生成/持久化、指纹、网卡枚举
    │   ├── discovery.rs   # mDNS 注册与监听 → peers 事件
    │   ├── protocol.rs    # 共享消息类型（prepare/chunk/finalize）
    │   ├── server.rs      # 接收端 HTTPS（axum+rustls）、会话、分块落盘、校验
    │   └── sender.rs      # 发送端（reqwest+证书钉扎）、分块上传、进度
    └── tests/transfer.rs  # 无 GUI 集成测试
```

## 协议摘要

```
POST /api/prepare   {files:[{name,size,sha256,chunk_size,chunks}], total}
                    → 长轮询对端用户决定（≤90s）
                    ← {accepted, token, resume:{fileIdx:[chunkIdx…]}}
PUT  /api/chunk/{token}/{fileIdx}/{chunkIdx}   body=4MB 原始字节
POST /api/finalize/{token}   → 对端流式 sha256 校验并落盘
```

- 指纹 = sha256(cert_der)（hex）；换网络重签证书时指纹随之更新并通过 mDNS 重新公布
- P1 计划：身份密钥与证书分离（指纹=公钥哈希）、TOFU 信任存储、WebRTC 跨网配对（见方案文档三通道模型）

## 已知限制（P0）

- 只发文件不发文件夹（拖入文件夹会跳过）
- 传输不支持中途暂停/取消 UI（断线自动可续传）
- 单网卡单地址选择，多网卡环境可能选到不通的地址（P1 改为多候选连接）
- 图标为占位品牌图，未做签名/公证（macOS 首次打开按 README 的「仍要打开 / xattr」步骤放行）
