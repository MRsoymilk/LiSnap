# LiSnap

使用 Rust 开发的跨平台截屏工具。当前版本 `0.1.0`，处于项目框架阶段，不依赖 Qt。

**当前没有真实截图功能。** 桌面窗口代码已建立，但所有平台捕获后端都是明确返回 `NotImplemented` 的占位实现；不会返回模拟桌面、空图片或虚假的成功结果。

## 技术结构

核心和平台接口仅依赖 Rust 标准库。桌面界面使用 egui/eframe，渲染使用 wgpu；Linux 截图计划直接接入 Wayland 协议，不提供 Portal 截图后端。Windows、macOS、X11 的原生实现分别隔离在平台模块中。

```text
LiSnap/
├── Cargo.toml                   # Workspace、版本、依赖与 lint 配置
├── apps/lisnap/src/              # 程序入口、CLI 参数和启动组合
├── crates/
│   ├── lisnap-core/              # 截图类型、不可变 RGBA8 图像、裁剪、坐标换算
│   ├── lisnap-platform/src/
│   │   ├── linux/wayland.rs      # 直连 Wayland 的后续实现入口
│   │   ├── linux/x11.rs          # X11 的后续实现入口
│   │   ├── windows/mod.rs        # Windows 原生后端入口
│   │   └── macos/mod.rs          # macOS 原生后端入口
│   └── lisnap-ui/                # egui/eframe 桌面窗口
├── scripts/check-core.sh        # Unix 离线核心/CLI 检查
├── docs/architecture.md          # 边界、坐标和捕获约定
└── plans/README.md               # 分阶段开发顺序
```

## 构建与启动

工具链要求：Rust/Cargo 1.97 或更新版本。没有写入 rust-toolchain 文件，不会替换本机工具链。首次 Cargo 解析需要访问依赖仓库；GUI 运行还需要可用的桌面会话和图形后端。

依赖可用后的常规命令：

```sh
cargo run -p lisnap
cargo run -p lisnap -- --backend wayland
cargo run -p lisnap -- --list-backends
cargo run -p lisnap --no-default-features -- --list-backends
```

不带参数时启动桌面框架窗口。`--backend` 接受 `auto`、`wayland`、`x11`、`windows`、`macos`；指定非本机平台的后端会报错。`--list-backends` 只显示后端清单，不打开窗口、不请求屏幕内容。

自动选择只依据操作系统及会话环境提示，不代表协议存在、权限获准或捕获可用。没有桌面会话时显示 `none`，不伪造显示器信息。

## 验证

常规 Cargo 验证：

```sh
cargo fmt --all -- --check
cargo test -p lisnap-core -p lisnap-platform -p lisnap --no-default-features
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Cargo 的 workspace 依赖解析仍可能访问可选 GUI 依赖的索引；`--no-default-features` 不是首次构建完全离线的保证。网络不可用时，可以直接验证没有第三方依赖的核心部分：

```sh
sh scripts/check-core.sh
./target/standalone-check/lisnap --list-backends
```

该脚本通过 rustc 编译核心库、平台库、19 个测试以及无 GUI 的 CLI，开启 `-D warnings` 和 `-D unsafe_code`。产物位于 `target/standalone-check/`。它不验证 eframe/wgpu、Cargo 特性解析或其他操作系统的原生实现，不可替代完整 Cargo 检查。

### 当前验证结果

在本机 Linux / rustc 1.97.1 上，Cargo 依赖已经解析并生成 `Cargo.lock`；格式检查、19 个离线测试、完整 workspace `cargo check`、`cargo clippy -D warnings` 以及带 GUI 特性的二进制编译/链接和 `--version` 运行均已通过。当前自动化执行环境没有 `WAYLAND_DISPLAY`、`WAYLAND_SOCKET` 或 `DISPLAY`，因此真正创建桌面窗口的运行验证仍待在图形会话中完成；Windows/macOS 构建验证也仍待完成。

## 当前范围

已实现 workspace、CLI、桌面窗口源码、平台接口与占位后端、只读像素数据、裁剪、负坐标/分数缩放选区换算及基础测试。未实现真实屏幕捕获、全屏冻结覆盖层、鼠标框选交互、标注、PNG 导出、剪贴板、全局快捷键和托盘。

下一步优先完成 **Wayland 协议能力探测与单显示器帧捕获**，然后接入多屏覆盖层。不要将普通桌面窗口当成已实现的跨平台截屏 Overlay。
