<p align="center">
  <img src="ui/brand/icon.png" width="112" height="112" alt="跃传 LeapDrop 图标">
</p>

# 跃传 LeapDrop

**简单好用的 Mac / Windows 局域网文件互传工具。**

同一网络，自动发现，拖入即传。无需账号、无需配对、无需上传网盘，文件直接在两台电脑之间传输。

[下载最新版](https://github.com/peng411528/leapdrop/releases/latest) · [查看所有版本](https://github.com/peng411528/leapdrop/releases)

## 适合什么时候用

- **Mac 与 Windows 互传**：把设计稿、文档、照片或视频，从 Mac 发给旁边的 Windows 电脑。
- **办公室临时协作**：同事在同一 Wi-Fi 下交换文件，省去建共享文件夹和上传网盘的步骤。
- **家里多台电脑之间传文件**：台式机和笔记本同时打开跃传，即可发送素材、资料或压缩包。
- **不方便使用云盘时**：连接同一局域网即可使用，文件传输不依赖互联网或第三方服务器。

## 三步开始

1. 两台电脑连接同一 Wi-Fi / 局域网，并打开跃传。
2. 在「设备」页选中对方电脑，拖入文件，或点击选择文件。
3. 接收方点击「保存到下载目录」，文件即开始传输。

**不注册、不配对、不填 IP 地址。** 接收到的文件保存在系统下载目录下的 `LeapDrop/` 文件夹。

## 下载安装

| 平台 | 下载文件 | 使用方式 |
| --- | --- | --- |
| macOS 11 及以上，Apple Silicon / Intel | `LeapDrop_0.3.1_macOS_universal.dmg` | 打开 DMG，将 LeapDrop 拖入「应用程序」 |
| Windows 10 / 11，64 位 | `LeapDrop_0.3.1_Windows_x64.zip` | 解压后双击 `leapdrop.exe`，无需安装 |

请在两台电脑上使用 v0.3.0 或更新的兼容版本。Release 中提供 `SHA256SUMS.txt`，可用于核对下载文件的完整性。

### 首次启动

- **macOS**：本版本采用本地 ad-hoc 签名，尚未经过 Apple 公证。系统拦截时，在「系统设置 → 隐私与安全性」中找到 LeapDrop，选择「仍要打开」。允许「本地网络」及防火墙传入连接。
- **Windows**：需要 Microsoft WebView2 运行时（多数 Windows 10 / 11 设备已安装）。缺少时从 [微软官网下载](https://developer.microsoft.com/microsoft-edge/webview2/)。防火墙提示时允许「专用网络」；未签名程序可能触发 SmartScreen，请核对下载来源。

## 核心功能

- **自动发现**：同一局域网内自动列出在线设备，无需手动输入地址。
- **直接发送**：发现即可发起传输，支持拖拽、多选、中文文件名和空文件。
- **自己决定是否接收**：「设置 → 允许接收文件」默认开启，自动记住上次选择；每次保存前仍需确认。
- **随时关闭接收**：关闭后自动拒绝新请求和待确认请求，仍可向其他设备发送；已开始的传输继续完成。
- **加密直传**：文件通过 HTTPS 在两台电脑之间传输，发送端核对发现信息中的证书指纹。
- **校验与进度**：显示发送 / 接收进度，以 SHA-256 校验文件内容，同名文件自动加序号。

跃传专注于局域网内传文件。当前不支持文件夹（可先压缩）、跨互联网传输、断点续传、暂停或取消。中断后需重新发送。设备名称来自对方自行声明，首次发现的设备没有经过身份认证，请在可信的局域网内使用，并核对接收请求。

## 开发运行

安装 Rust stable。macOS 需要 Xcode Command Line Tools；Windows 需要 Visual Studio C++ Build Tools 和 WebView2 运行时。前端为原生 HTML / CSS / JavaScript，无需 Node 构建或安装 npm 依赖。

在项目根目录执行：

```sh
cargo run -p leapdrop
cargo test --workspace --locked
cargo fmt --all -- --check
```

只编译本机可执行文件：

```sh
cargo build -p leapdrop --release --locked
```

发布打包使用 `scripts/package-macos.sh`、`scripts/package-windows.ps1`；Mac 交叉编译 Windows 可用 `scripts/package-windows-cross.sh`。详见 [开发约定](docs/development.md)。所有编译文件与安装包都输出到根目录 `target/`，最终分发文件统一放 `target/release-artifacts/`，不提交 Git。

本机若因新 macOS SDK 出现 `tapi error`，可临时指定已安装的兼容 SDK，例如：

```sh
SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk cargo run -p leapdrop
```

## 项目结构

```text
leapdrop/
├── Cargo.toml           # 单应用 workspace、发布编译配置
├── Cargo.lock           # 唯一依赖锁文件，需提交
├── README.md
├── docs/
│   └── development.md  # 文件管理、模块职责、验证流程
├── scripts/             # Mac / Windows 发布打包脚本
├── ui/                  # 界面源码，Tauri 直接加载
│   ├── index.html
│   ├── style.css
│   └── app.js
└── src-tauri/
    ├── Cargo.toml
    ├── build.rs
    ├── tauri.conf.json   # 桌面窗口与打包配置
    ├── Info.plist        # macOS 本地网络权限声明
    ├── capabilities/     # Tauri 窗口权限
    ├── icons/            # 应用图标源资源
    ├── src/              # Rust 设备发现、身份、发送、接收逻辑
    └── tests/transfer.rs # 无 GUI 的真实 HTTPS 传输测试
```

## 使用与排查

1. 两台电脑连接同一 Wi-Fi / 网段，均启动应用。
2. macOS 允许「本地网络」和防火墙传入连接；Windows 防火墙允许「专用网络」。
3. 选择对方设备，拖入文件；接收方确认保存。

扫描不到时检查访客网络、AP 隔离、VPN 与多网卡。发现使用 UDP 5353，文件接收优先使用 TCP 52920，端口占用时自动分配并通过 mDNS 公布。当前对每台设备只选择一个连接地址，多网卡环境可能选到不可达地址。

设备身份及接收偏好保存在系统应用数据目录（标识 `com.leapdrop.app`），接收暂存文件位于下载目录 `LeapDrop/.leapdrop-tmp/`，均不写入项目源码。未完成会话运行中约 30 分钟后清理；应用异常退出后遗留的暂存目录可手动删除。

## 开源协议

本项目采用 [MIT License](LICENSE)。版权声明和许可全文随源码及 Mac / Windows 软件包一同提供。
