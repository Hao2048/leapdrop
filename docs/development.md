# 开发约定

## 范围与模块

本项目只维护 macOS / Windows 的同一局域网设备发现及文件发送接收。新增需求应在这条核心链路上开发。

| 文件 | 职责 |
| --- | --- |
| `ui/index.html` | 设备、传输、设置页面及确认弹窗 |
| `ui/style.css` | 界面样式 |
| `ui/app.js` | 界面状态、Tauri 命令与事件 |
| `src-tauri/src/lib.rs` | 应用启动、共享状态、命令和事件接线 |
| `src-tauri/src/discovery.rs` | mDNS 注册与设备发现 |
| `src-tauri/src/identity.rs` | 本机设备身份、证书与网络地址 |
| `src-tauri/src/settings.rs` | 接收偏好的默认值、持久化与待确认请求关闭 |
| `src-tauri/src/protocol.rs` | 发送和接收共用的协议类型 |
| `src-tauri/src/sender.rs` | 发起传输、分块上传与发送进度 |
| `src-tauri/src/server.rs` | HTTPS 接收服务、用户确认、校验和文件保存 |
| `src-tauri/tests/transfer.rs` | 真实回环 HTTPS 集成测试 |

传输流程：mDNS 发现 → `POST /api/prepare` 等待接收确认 → `PUT /api/chunk/{token}/{file}/{idx}` 逐块上传 → `POST /api/finalize/{token}` 校验并保存。

每次发送创建独立临时会话；不恢复上次未完成的传输。保留固定分块以限制传输内存占用，不维护续传 bitmap。

## 文件管理

- 根目录只放入口说明、workspace 配置及唯一的 `Cargo.lock`。
- UI、Rust 代码、测试、文档、图标分别放在既有目录；不要堆放到根目录。
- 编译缓存、可执行文件和安装包统一放根目录 `target/`。Tauri 自动生成的 `src-tauri/gen/` 也属于可删除产物。
- `dist/` 如用于手动导出，同样不提交 Git；不得在源码目录保留旧安装包或备份副本。
- 机器专用 SDK 路径使用临时 `SDKROOT`，或放被忽略的本机 `.cargo/config.toml`，不要硬编码到公共配置。
- 应用运行数据使用系统应用数据 / 下载目录；测试数据使用系统临时目录并在测试结束时清理。
- 保留 `.git/` 版本历史。调整目录时同步更新 README 和路径引用。

清理所有构建与打包文件可在根目录执行 `cargo clean`；自动生成的 `src-tauri/gen/` 可一并删除。再次构建会重新生成所需文件。

## 验证

在根目录运行：

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo build -p leapdrop --locked
```

本机有 Node 时，可额外执行 `node --check ui/app.js` 检查前端语法。

集成测试覆盖多文件、多分块、中文设备名与文件名、空文件、重名保护、接收拒绝、接收开关的默认值与持久化、关闭时拒绝待确认请求、保存设置失败时保持原值、证书指纹不匹配。测试创建真实 HTTPS 接收端，不打开 GUI。

跨机交付前需要一台 Mac 和一台 Windows 在同一局域网双向实测：自动发现、免配对发送、接收开关、接受与拒绝、多文件内容一致、设备离线。单机回环测试不能替代网络权限、防火墙和 Windows 实机验收。

## 清理记录（2026-10-19）

已移除 Android / iOS 代码与生成工程、WebRTC 信令服务、iOS 修复脚本、旧发布工作流与副本、多余 lock 文件、无调用的探测接口，以及失效的续传 bitmap 逻辑。旧构建缓存和安装包均不保留。

## 发布打包

版本号需同步更新 `src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、界面关于信息与发布说明，并更新根目录 lock 文件。

- Mac 通用 DMG：`bash scripts/package-macos.sh`。需要 Tauri CLI、两个 Mac Rust target；脚本先生成 .app，再进行 ad-hoc 签名并验证 DMG。
- Windows 本机构建：`powershell -File scripts/package-windows.ps1`。
- 从 Mac 交叉编译 Windows：`LLVM_BIN=/path/to/llvm/bin bash scripts/package-windows-cross.sh`。需要 cargo-xwin 与 LLVM；本机 SDK 不兼容时同样通过 `SDKROOT` 指定可用的 Mac SDK。
- 最终分发文件统一放 `target/release-artifacts/`，只上传到 GitHub Release，不提交 Git。校验清单为该目录内分发文件的 SHA-256。
- `docs/quick-start.txt` 随 Mac / Windows 包一起分发；对应版本的发布说明放 `docs/releases/`。
