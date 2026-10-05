# LiSnap

使用 Rust 开发的跨平台截屏工具。当前版本 `0.1.0`，不依赖 Qt。

Linux/Wayland 已实现直接启动截图的交互链路：优先使用 `ext-image-copy-capture`，不可用时回退 `wlr-screencopy`；冻结屏幕与选区编辑使用 `zwlr_layer_shell_v1`、SHM、`wp_viewporter` 和 `wl_subcompositor`，不经过 Portal。完成选区后显示浮动工具条，可使用自由画笔、矩形、箭头、文字、撤销/重做、确认/取消；确认后输出带标注的 PNG。X11、Windows、macOS 仍是明确失败的占位实现。Wayland 编辑器仍需在宿主 Hyprland 多屏会话完成真机验收。

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
cargo run -p lisnap -- --output ~/Pictures/LiSnap/manual.png
cargo run -p lisnap -- --screen DP-1
cargo run -p lisnap -- --window
cargo run -p lisnap -- --list-backends
cargo run -p lisnap --no-default-features -- --list-backends
cargo run -p lisnap --no-default-features -- --list-screens
cargo run -p lisnap --no-default-features -- --probe-wayland
cargo run -p lisnap --no-default-features -- --capture-wayland
```

不带参数时直接进入交互式截图，不先打开普通主窗口。在 Hyprland 多屏环境中会读取当前活动 monitor，只捕获对应 `wl_output`；也可以用 `--screen OUTPUT` 显式指定。`--window` 保留原来的开发/诊断桌面窗口。`--output PATH` 指定确认后的 PNG 路径；未指定时保存到 `~/Pictures/LiSnap/`。`--backend` 接受 `auto`、`wayland`、`x11`、`windows`、`macos`，但当前交互式编辑器只实现 Wayland。

截图流程为：捕获当前输出 → 显示冻结画面 → 拖动选区 → 浮动工具条编辑 → 确认保存 PNG。工具条当前包含画笔、矩形、箭头、文字、Undo、Redo、确认和取消；快捷键支持 `P/R/A/T` 切换工具、`Ctrl+Z/Ctrl+Y` 撤销/重做、Enter 确认、Esc 取消。文字输入第一版支持英文、数字及常用 ASCII 符号；Wayland IME/中文输入尚未接入。

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

在本机 Linux / rustc 1.97.1 上，完整 workspace `cargo check --all-targets`、平台/CLI/UI 测试以及 `cargo clippy --workspace --all-targets -- -D warnings` 均已通过。Wayland 捕获覆盖 registry/output 枚举、ext/wlr manager、SHM 单帧复制、ARGB/XRGB、stride、Y-invert/transform、cursor option、超时和取消；交互编辑器已覆盖选区、工具条布局/命中、CPU 标注合成、PNG 输出入口和 Hyprland monitor 字段解析。冻结帧与工具条使用原生 layer-shell/subsurface，标注采用双 SHM buffer 更新，最终直接合成到原始捕获像素后裁剪输出。当前 EndlessVibe bubblewrap 没有挂载宿主 `/run/user/1000/wayland-*`，因此真实多屏目标选择、工具条显示、鼠标/键盘输入与保存结果仍必须在宿主 Hyprland 会话最终验收。Windows/macOS 本地交叉检查因对应 Rust target 未安装而无法完成。

## 当前范围

已实现 workspace、CLI、开发诊断窗口、Linux/Wayland 直接启动截图、Hyprland 活动输出定位、冻结 Overlay、拖选、浮动工具条、自由画笔、矩形、箭头、基础文字、撤销/重做、确认/取消、原图标注合成、PNG 导出、负坐标/分数缩放选区换算及基础测试。X11、Windows、macOS 交互截图仍未实现；Wayland 宿主真机验收、中文/IME 文字输入、剪贴板、全局快捷键、托盘和跨屏选区仍待开发。

下一步优先完成 **Hyprland 多屏下直接启动截图 + 工具条标注的真机验收**，随后补中文/IME 与剪贴板。不要把自动化环境中的编译/单测当成真实 compositor 验收。
