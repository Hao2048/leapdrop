# 跃传 LeapDrop

[![release](https://github.com/peng411528/leapdrop/actions/workflows/release.yml/badge.svg)](../../actions)

跨平台局域网文件互传（macOS / Windows）。**mDNS 自动发现 + TLS 直传 + 接收确认 + 断点续传**，无需服务器、文件不出局域网。

## 下载

到 [Releases](../../releases) 页下载对应平台安装包：

| 平台 | 文件 | 首次打开 |
|---|---|---|
| macOS (Apple Silicon / Intel) | `LeapDrop_x.x.x_universal.dmg` | 未签名：右键 → 打开（或 系统设置 → 隐私与安全性 → 仍要打开） |
| Windows | `LeapDrop_x.x.x_x64-setup.exe` | 未签名：SmartScreen 提示时点「仍要运行」 |

首次运行需要允许：macOS 的「本地网络」权限与防火墙；Windows 防火墙（勾选专用网络）。

## 功能（P0）

- 同一局域网内自动发现设备（mDNS / `_leapdrop._tcp.local.`），雷达式设备列表
- 拖拽 / 文件对话框发送，接收方弹窗确认后才开始传输
- 全程 TLS：自签证书 + **证书指纹钉扎**（mDNS TXT 公布指纹，发送方校验对端证书一致，防局域网中间人）
- 4MB 分块并行度友好传输、sha256 整文件校验、断点续传（bitmap 边车）
- 接收文件保存到 `~/Downloads/LeapDrop`，重名自动加序号

## 构建与运行

前置：Rust（rustup stable）、macOS 需要 Xcode Command Line Tools；Windows 需要 VS Build Tools。

```bash
cd src-tauri
cargo run        # 开发运行（首次编译约几分钟）
cargo test       # 集成测试：TLS 回环传输 + 拒绝流程
cargo build --release   # 发布构建（产物在 target/release/leapdrop）
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
- 图标为占位品牌图，未做签名/公证（macOS 首次打开需右键→打开）
