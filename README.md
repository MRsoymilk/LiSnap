# LiSnap

使用 Rust 开发的跨平台截屏工具。当前版本 `0.1.0`，处于项目框架阶段，不依赖 Qt。

Linux/Wayland 已实现真实单帧捕获：优先使用 `ext-image-copy-capture`，不可用时回退 `wlr-screencopy`，两条路径都直接使用 SHM，不经过 Portal。X11、Windows、macOS 仍是明确失败的占位实现；不会返回模拟桌面、空图片或虚假的成功结果。

## 技术结构

核心和平台接口仅依赖 Rust 标准库。桌面界面使用 egui/eframe，渲染使用 wgpu；Linux 截图计划直接接入 Wayland 协议，不提供 Portal 截图后端。Windows、macOS、X11 的原生实现分别隔离在平台模块中。

```text
LiSnap/
├── Cargo.toml                   # Workspace、版本、依赖与 lint 配置
├── apps/lisnap/src/              # 程序入口、CLI 参数和启动组合
├── crates/
│   ├── lisnap-core/              # 截图类型、不可变 RGBA8 图像、裁剪、坐标换算
│   ├── lisnap-platform/src/
│   │   ├── linux/wayland.rs      # Wayland 探测、协议选择与统一捕获入口
│   │   ├── linux/x11.rs          # X11 的后续实现入口
│   │   ├── windows/mod.rs        # Windows 原生后端入口
│   │   └── macos/mod.rs          # macOS 原生后端入口
│   └── lisnap-ui/                # egui/eframe 桌面窗口
├── scripts/check-core.sh        # Unix 离线核心/CLI 检查
├── scripts/validate-wayland.sh  # 真实 Wayland 会话探测与帧捕获验收
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
cargo run -p lisnap --no-default-features -- --list-screens
cargo run -p lisnap --no-default-features -- --probe-wayland
cargo run -p lisnap --no-default-features -- --capture-wayland
```

不带参数时启动桌面框架窗口。`--backend` 接受 `auto`、`wayland`、`x11`、`windows`、`macos`；指定非本机平台的后端会报错。`--list-backends` 只显示后端清单；`--list-screens` 通过选定的 `CaptureBackend::screens()` 输出稳定 screen ID、名称、像素尺寸和 scale，不捕获屏幕像素。

自动后端候选仍只依据操作系统及会话环境提示；真正捕获时会连接 Wayland registry 并按实际绑定能力选择 `ext-image-copy-capture` 或 `wlr-screencopy`。没有桌面会话、协议缺失、权限拒绝或帧约束不兼容时会明确失败，不伪造显示器信息。

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

该脚本通过 rustc 编译核心库、平台库和无 GUI CLI 的离线子集，开启 `-D warnings` 和 `-D unsafe_code`。产物位于 `target/standalone-check/`。它不验证 Wayland 第三方协议依赖、eframe/wgpu、Cargo 特性解析或其他操作系统的原生实现，不可替代完整 Cargo 检查。

真实 Wayland 捕获必须在能够访问 compositor socket 的桌面会话中验证：

```sh
sh scripts/validate-wayland.sh
# 指定输出时：
LISNAP_SCREEN=DP-1 sh scripts/validate-wayland.sh
# 服务/容器环境可显式指定 socket：
LISNAP_WAYLAND_SOCKET=/run/user/1000/wayland-1 sh scripts/validate-wayland.sh
```

脚本先执行 `--probe-wayland`，随后执行 `--capture-wayland`。成功时输出每个捕获帧的输出 ID、像素尺寸、RGBA 字节数和 FNV64 校验和；它不打开截图选择 UI，也不会写入截图文件。

### 当前验证结果

在本机 Linux / rustc 1.97.1 上，完整 workspace `cargo check --all-targets`、平台/CLI/UI 测试、`cargo clippy --workspace --all-targets -- -D warnings` 以及 GUI 启动验证均已通过。Wayland 代码现已覆盖 registry/output 枚举、ext/wlr manager 绑定、SHM 单帧复制、ARGB/XRGB 转换、stride、Y-invert/transform、cursor option、5 秒超时和取消；桌面窗口通过独立 worker 自动枚举输出，可选择单个输出或全部输出，Capture/Cancel 已接入 native backend，并能把首帧 RGBA 数据显示为静态 egui texture 预览。当前自动化执行环境的 bubblewrap 没有挂载宿主 `/run/user/1000/wayland-*`，因此真实桌面像素仍必须通过 `scripts/validate-wayland.sh` 在宿主 Wayland 会话中完成最终验收。Windows/macOS 本地交叉检查因对应 Rust target 未安装而无法完成；仓库已加入原生 GitHub Actions 构建矩阵，仍需等待远端实际运行后验收。

## 当前范围

已实现 workspace、CLI、桌面窗口、后台输出枚举与 Capture/Cancel worker、输出选择、首帧静态预览、平台接口、Linux/Wayland 原生单帧捕获、只读像素数据、裁剪、负坐标/分数缩放选区换算及基础测试。X11、Windows、macOS 捕获仍未实现；全屏冻结覆盖层、鼠标框选交互、标注、PNG 导出、剪贴板、全局快捷键和托盘也仍待开发。

下一步先完成 **Wayland 真机像素验收**，确认实际 compositor 输出可读且不包含自身选择 UI；之后进入静态覆盖层与选区阶段。不要将普通桌面窗口当成已实现的跨平台截屏 Overlay。
