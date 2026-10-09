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
│   ├── lisnap-core/              # 截图类型、不可变 RGBA8 图像、裁剪、坐标换算与持久化设置
│   ├── lisnap-platform/src/
│   │   ├── linux/wayland.rs      # Wayland 探测、捕获、layer-shell Overlay 入口
│   │   ├── linux/x11.rs          # X11 的后续实现入口
│   │   ├── windows/mod.rs        # Windows 原生后端入口
│   │   └── macos/mod.rs          # macOS 原生后端入口
│   └── lisnap-ui/                # egui/eframe 诊断窗口与独立设置页
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
cargo run -p lisnap -- --settings
cargo run -p lisnap -- --include-cursor
cargo run -p lisnap -- --no-cursor
cargo run -p lisnap -- --window
cargo run -p lisnap -- --list-backends
cargo run -p lisnap --no-default-features -- --list-backends
cargo run -p lisnap --no-default-features -- --list-screens
cargo run -p lisnap --no-default-features -- --probe-wayland
cargo run -p lisnap --no-default-features -- --capture-wayland
```

不带参数时直接进入交互式截图，不先打开普通主窗口。在 Hyprland 多屏环境中会读取当前活动 monitor，只捕获对应 `wl_output`；也可以用 `--screen OUTPUT` 显式指定。`--settings`（也支持 `lisnap settings`）单独启动图形化设置页，不会先截图。设置页的 **Show mouse cursor in screenshots** 开关会立即保存，控制捕获图像是否包含鼠标指针，默认关闭。配置文件为 `$XDG_CONFIG_HOME/lisnap/config.toml`（未设置 XDG_CONFIG_HOME 时为 `~/.config/lisnap/config.toml`），内容示例：`[capture]` 下的 `include_cursor = true`。默认截图、Wayland 捕获诊断和 `--window` 诊断界面的初始值都会读取该配置；`--include-cursor` / `--no-cursor` 只覆盖当次截图，不改变配置。Wayland 的 `ext-image-copy-capture` 和 `wlr-screencopy` 均可向 compositor 请求包含/隐藏鼠标指针，实际效果取决于 compositor。`--window` 保留原来的开发/诊断桌面窗口。`--output PATH` 指定确认后的 PNG 路径；未指定时保存到 `~/Pictures/LiSnap/`。`--backend` 接受 `auto`、`wayland`、`x11`、`windows`、`macos`，但当前交互式编辑器只实现 Wayland。

截图流程为：捕获当前输出 → 显示冻结画面 → 拖动选区 → 浮动工具条编辑 → 确认保存 PNG。工具条最左侧是颜色选择（12 种预设颜色 + 自定义颜色），其后依次为画笔、矩形、箭头、文字、马赛克、Undo、Redo、保存、复制、确认和取消；保存将带标注选区写入 PNG，复制则通过 `wl-copy --type image/png` 将同一张 PNG 放入 Wayland 剪贴板而不写文件（Gentoo 需安装 `gui-apps/wl-clipboard`）；快捷键支持 `P/R/A/T` 切换基本工具、`Ctrl+Z/Ctrl+Y` 撤销/重做、Enter 确认、Esc 取消。参考 Snipaste 的选中后再编辑交互：点击已有画笔、箭头、矩形、文字或马赛克，即可将其设为当前唯一选中对象，外边缘出现青色虚线选框；拖动对象或选框边缘可移动，点击其他对象切换选中，点击空白区域或按 Esc 取消选中。选中箭头时，首尾两端显示白色圆形控制点，可分别拖动起点或终点调整长度和方向；拖动箭头中段移动整支箭头。**双击已有文字**可重新编辑其内容（保留颜色、字号和原有层级），按 Enter 或点击其他位置提交，编辑时按 Esc 撤销本轮文字修改。修改文字后可继续选中并拖动。编辑器会把文字修改、删除及箭头调整记录到 `Ctrl+Z/Ctrl+Y` 历史中，外侧选框和端点控制点均为预览元素，不会导出到 PNG。马赛克工具拖动框选区域后以像素块替换原截图中对应区域，导出的 PNG 直接保存已遮挡的像素（不是可以通过隐藏图层恢复的非破坏性叠层）；这是像素化遮挡，并非密码学加密。Text 工具输入期间可按 `Ctrl+A` 全选当前文字（高亮），随后输入新内容或按 Backspace/Delete 替换/删除；`Ctrl+Z/Ctrl+Y` 优先撤销/重做当前文字的逐次编辑，退出文字输入后恢复标注级撤销/重做。右键工具栏上的画笔、矩形、箭头图标（或在选区内使用这些工具时右键），可以选择 1/2/3/5/8/12 px 线宽；右键文字工具则可选择 16/20/24/32/40/48 px 字号；右键马赛克工具可选择 8/12/18/24/32/48 px 方块大小。点击最左侧颜色块打开颜色面板，前 12 项为预设色，最后一项为 CUSTOM（自定义颜色）。点击 CUSTOM 后可在二维颜色矩形中单击或拖动取色：横向为饱和度（左白右纯色）、纵向为明度（上亮下黑），下方彩虹色相条可切换基色；选中位置有可见指示标记。仍可通过 R/G/B 三条滑轨或 HEX 输入框输入 6 位十六进制颜色（如 #2D8BC7），各种方式的数值互相同步。点击 APPLY 确认，BACK 或 Esc 返回预设面板。选色和 R/G/B 拖动时实时显示颜色预览，自定义色会以色块保留在 CUSTOM 项（当前截图编辑会话内）。新的选择用于后续画笔、矩形、箭头和文字，以及正在编辑的草稿，已完成的标注保留原颜色。选项菜单显示当前值，左键点选后关闭，也可点击外部或按 Esc 关闭；参数用于当前文字草稿和后续标注，已完成的标注保留原始线宽/字号。Wayland 文字工具已接入 `text-input-v3`，可接收 fcitx5 等输入法的预编辑/提交字符串；文字使用系统 fontconfig 中文字体渲染，也可通过 `LISNAP_FONT=/path/to/font.ttf` 指定字体。

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

已实现 workspace、CLI、开发诊断窗口、Linux/Wayland 直接启动截图、Hyprland 活动输出定位、冻结 Overlay、拖选、浮动工具条、自由画笔、矩形、箭头、Unicode/中文文字、Wayland text-input-v3 IME、撤销/重做、确认/取消、原图标注合成、PNG 导出、负坐标/分数缩放选区换算及基础测试。X11、Windows、macOS 交互截图仍未实现；Wayland 宿主真机验收、剪贴板、全局快捷键、托盘和跨屏选区仍待开发。

下一步优先完成 **Hyprland 多屏下直接启动截图 + 工具条标注 + fcitx5 中文输入的真机验收**，随后补剪贴板。不要把自动化环境中的编译/单测当成真实 compositor 验收。
