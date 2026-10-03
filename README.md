# LiSnap

使用 Rust 开发的跨平台截屏工具。当前版本 `0.1.0`，处于项目框架阶段，不依赖 Qt。

Linux/Wayland 已实现真实单帧捕获与全屏选区 Overlay 代码链路：优先使用 `ext-image-copy-capture`，不可用时回退 `wlr-screencopy`；冻结选区使用 `zwlr_layer_shell_v1`、SHM、`wp_viewporter` 和 `wl_subcompositor`，不经过 Portal。X11、Windows、macOS 仍是明确失败的占位实现；不会返回模拟桌面、空图片或虚假的成功结果。Wayland Overlay 仍需在宿主 Hyprland 会话完成真机验收。

## 技术结构

核心和平台接口仅依赖 Rust 标准库。桌面界面使用 egui/eframe，渲染使用 wgpu；Linux 截图计划直接接入 Wayland 协议，不提供 Portal 截图后端。Windows、macOS、X11 的原生实现分别隔离在平台模块中。

```text
LiSnap/
├── Cargo.toml                   # Workspace、版本、依赖与 lint 配置
├── apps/lisnap/src/              # 程序入口、CLI 参数和启动组合
├── crates/
│   ├── lisnap-core/              # 截图类型、不可变 RGBA8 图像、裁剪、坐标换算
│   ├── lisnap-platform/src/
│   │   ├── linux/wayland.rs      # Wayland 探测、捕获、layer-shell Overlay 入口
│   │   ├── linux/x11.rs          # X11 的后续实现入口
│   │   ├── windows/mod.rs        # Windows 原生后端入口
│   │   └── macos/mod.rs          # macOS 原生后端入口
│   └── lisnap-ui/                # egui/eframe 桌面窗口
├── scripts/check-core.sh        # Unix 离线核心/CLI 检查
├── scripts/validate-wayland.sh  # 真实 Wayland 会话探测与帧捕获验收
├── scripts/validate-wayland-overlay.sh # 冻结 Overlay/拖选/裁剪宿主验收
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

全屏冻结选区可独立验收，不依赖主 GUI：

```sh
sh scripts/validate-wayland-overlay.sh
LISNAP_SCREEN=DP-1 sh scripts/validate-wayland-overlay.sh
```

该脚本调用 `lisnap-platform` 的 `wayland-overlay` example：先 headless 捕获目标输出，再显示冻结 layer-shell Overlay；左键拖拽确认选区、Esc 取消。确认后会对原始 `RgbaImage` 执行裁剪并输出选区坐标、尺寸、RGBA 字节数和 FNV64。

### 当前验证结果

在本机 Linux / rustc 1.97.1 上，完整 workspace `cargo check --all-targets`、平台/CLI/UI 测试、`cargo clippy --workspace --all-targets -- -D warnings` 以及 GUI 启动验证均已通过。Wayland 捕获覆盖 registry/output 枚举、ext/wlr manager、SHM 单帧复制、ARGB/XRGB、stride、Y-invert/transform、cursor option、超时和取消；桌面窗口通过 worker 自动枚举/捕获输出。S2 代码链路也已接通：`CaptureFrame` 可映射为全屏 layer-shell 冻结帧，分数缩放由 `wp_viewporter` 处理，拖动时由无输入区域的 subsurface 绘制半透明遮罩/白色边框，Esc 取消，返回 `PixelRect` 后 UI 自动裁原图。当前自动化 bubblewrap 没有挂载宿主 `/run/user/1000/wayland-*`，因此真实桌面像素和 Overlay 显示/输入仍必须分别通过 `scripts/validate-wayland.sh` 与 `scripts/validate-wayland-overlay.sh` 在宿主 Hyprland 会话最终验收。Windows/macOS 本地交叉检查因对应 Rust target 未安装而无法完成；仓库已加入原生 GitHub Actions 构建矩阵，仍需等待远端实际运行后验收。

## 当前范围

已实现 workspace、CLI、桌面窗口、后台输出枚举与 Capture/Cancel worker、输出选择、首帧静态预览、Linux/Wayland 原生单帧捕获、全屏冻结 Overlay/拖选/Esc 取消的代码链路、原图裁剪、负坐标/分数缩放选区换算及基础测试。X11、Windows、macOS 捕获仍未实现；Wayland Overlay 的宿主真机验收、多输出/跨屏选区、标注、PNG 导出、剪贴板、全局快捷键和托盘仍待开发。

下一步优先完成 **Wayland 捕获与冻结 Overlay 的宿主 Hyprland 真机验收**；通过后再扩展多输出/跨屏选区。不要把自动化环境中的编译/单测当成真实 compositor 验收。
