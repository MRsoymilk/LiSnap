<!-- LiSnap | Primary language: 简体中文 -->
<p align="center">
  <img src="docs/images/logo.png" alt="LiSnap logo" width="136" />
</p>

<h1 align="center">LiSnap</h1>

<p align="center">
  <strong>轻快、原生、可标注的截图工具</strong><br />
  面向 Linux / Wayland，使用 Rust 构建。
</p>

<p align="center">
  <strong>简体中文</strong> · <a href="README.en.md">English</a>
</p>

<p align="center">
  <img alt="Version 0.1.0" src="https://img.shields.io/badge/version-0.1.0-2563eb" />
  <img alt="Rust 1.97+" src="https://img.shields.io/badge/Rust-1.97%2B-f97316" />
  <img alt="Linux Wayland" src="https://img.shields.io/badge/Linux-Wayland-0891b2" />
  <img alt="Development status: Alpha" src="https://img.shields.io/badge/status-alpha-64748b" />
</p>

LiSnap 是一个正在开发中的桌面截图与标注工具：**截取屏幕 → 框选区域 → 直接标注 → 保存 PNG 或复制图片**。当前可用的交互式截图后端为 **Linux / Wayland**，优先使用 `ext-image-copy-capture`，不支持时尝试 `wlr-screencopy`；不经由截图 Portal。X11、Windows 和 macOS 原生后端仍在规划中，不应视为已支持。

## 界面预览

### 选区与标注

![LiSnap 截图编辑器界面预览：选区、标注与底部工具栏](docs/images/editor-preview.jpg)

### 颜色面板

![LiSnap 颜色面板界面预览](docs/images/palette-preview.jpg)

> **预览说明：** 以上图片中的工具栏、标注和颜色面板由 **LiSnap 当前 Rust 原生绘制代码离屏渲染**，背景为演示场景，**不是 Hyprland 实机截图**。当前自动化执行环境无法连接宿主 Wayland socket。真实桌面截图仍需在 Hyprland 会话中采集与验收；参见[实机截图与验证](#实机截图与验证)。

## 功能亮点

- **原生 Wayland 截图：** 自动定位 Hyprland 活动显示器，也可通过 `--screen` 指定输出；捕获后显示冻结画面和可拖动的矩形选区。
- **就地标注：** 画笔、矩形、箭头、文字和马赛克；支持移动已有标注、调节箭头端点、双击重新编辑文字，以及撤销/重做。
- **灵活的绘图样式：** 12 种预设颜色、自定义 HSV 取色、RGB/HEX 输入；线宽、文字大小和马赛克块尺寸可调。
- **保存与复制分离：** 保存/确认输出已合成标注的 PNG；复制通过 `wl-copy` 将 PNG 放入 Wayland 剪贴板，不额外写入图片文件。
- **鼠标与文字输入：** 截图默认不包含鼠标指针，但截图界面中的鼠标仍正常显示和操作；支持 Unicode 文字与 Wayland `text-input-v3` 输入法交互（具体取决于 compositor / IME）。

> LiSnap 仍处于 **Alpha 阶段**：协议探测、数据处理和编辑器已有自动化验证，但多显示器、实际剪贴板粘贴及输入法行为仍需要在宿主 Hyprland 会话中验收。

## 平台支持

| 平台 | 当前状态 | 说明 |
| --- | --- | --- |
| Linux / Wayland | **已实现，待完整实机验收** | 面向提供所需截图协议与 layer-shell 能力的 compositor，优先 Hyprland |
| Linux / X11 | 规划中 | 当前仅占位后端 |
| Windows | 规划中 | 尚无可用原生截图实现 |
| macOS | 规划中 | 尚无可用原生截图实现 |

## 快速开始

**前提条件：** Rust / Cargo **1.97+**、受支持的 Linux Wayland 会话，以及能够访问 `ext-image-copy-capture` 或 `wlr-screencopy` 的 compositor。复制图片需要系统安装 `wl-copy`（来自 `wl-clipboard`）。Gentoo 可使用：

```bash
emerge -av gui-apps/wl-clipboard
```

克隆并启动：

```bash
git clone https://github.com/MRsoymilk/LiSnap.git
cd LiSnap
cargo run -p lisnap
```

不带参数时**直接进入交互式截图**，不会先打开普通主窗口。框选后即可标注，并使用底部保存、复制、确认或取消按钮。默认保存路径为 `~/Pictures/LiSnap/`。

常用命令：

```bash
# 交互式截图
cargo run -p lisnap

# 保存到指定 PNG 文件
cargo run -p lisnap -- --output ~/Pictures/LiSnap/example.png

# 指定显示器（例如 DP-1）
cargo run -p lisnap -- --screen DP-1

# 独立打开设置窗口
cargo run -p lisnap -- --settings

# 仅覆盖当前捕获的鼠标指针设置
cargo run -p lisnap -- --no-cursor
cargo run -p lisnap -- --include-cursor

# 查看后端、探测 Wayland 协议
cargo run -p lisnap -- --list-backends
cargo run -p lisnap -- --probe-wayland

# 打开开发/诊断界面（不是正常截图入口）
cargo run -p lisnap -- --window
```

更多参数使用 `cargo run -p lisnap -- --help`。

## 编辑操作

| 操作 | 交互方式 |
| --- | --- |
| 框选区域 | 在冻结画面上按住鼠标左键拖动 |
| 画笔 / 矩形 / 箭头 / 文字 | 工具栏按钮，或快捷键 `P` / `R` / `A` / `T` |
| 马赛克 | 选择工具栏中的马赛克图标，拖动框选遮挡区域 |
| 编辑已有对象 | 单击选中并拖动；箭头可拖动首尾控制点；双击已有文字重新输入 |
| 更改样式 | 左侧颜色按钮；右键工具按钮设置线宽、字号或马赛克尺寸 |
| 撤销 / 重做 | `Ctrl+Z` / `Ctrl+Y` |
| 保存 PNG | 点击保存或确认（`Enter`）；`--output` 可指定输出文件 |
| 复制到剪贴板 | 点击复制图标；需要 `wl-copy` |
| 取消 | 点击取消，或使用 `Esc`（输入/对象选中状态下可能先取消当前操作） |

文字输入期间支持 `Ctrl+A` 全选及逐次撤销/重做；中文输入使用系统字体与 compositor 提供的 `text-input-v3` 协议。马赛克对输出像素进行直接像素化处理，**并非加密**。

## 设置

运行 `cargo run -p lisnap -- --settings` 可调整并自动保存截图是否包含鼠标指针。默认关闭，对应配置：

```toml
# ~/.config/lisnap/config.toml
[capture]
include_cursor = false
```

如果配置了 `XDG_CONFIG_HOME`，文件位于 `$XDG_CONFIG_HOME/lisnap/config.toml`。命令行 `--include-cursor` / `--no-cursor` 仅影响本次捕获，不会修改已保存的设置。

## 实机截图与验证

目前提交的是**原生绘制器界面预览**。要补充或替换为实际桌面截图，请在可运行 LiSnap 的 Hyprland 会话中完成选区并打开标注工具栏，再从另一个终端采集显示器，例如：

```bash
# 在有权限访问同一 Wayland 会话的终端执行
grim -o DP-1 docs/images/editor-real.png
```

随后在确认图片不包含隐私信息后，将真实截图加入仓库并替换上方的预览引用。输出名称可通过本机 `hyprctl monitors` 查询。

构建、测试和代码检查：

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test -p lisnap-core -p lisnap-platform -p lisnap --no-default-features
cargo clippy --workspace --all-targets -- -D warnings
```

需要真实 Wayland socket 的捕获/冻结选区测试，应在宿主会话执行：

```bash
sh scripts/validate-wayland.sh
sh scripts/validate-wayland-overlay.sh
```

## 项目结构

| 目录 | 职责 |
| --- | --- |
| `apps/lisnap` | 命令行入口、截图流程和 PNG 输出 / 剪贴板复制 |
| `crates/lisnap-core` | 图像、选区、捕获请求与设置数据模型 |
| `crates/lisnap-platform` | 原生后端、Wayland 捕获与原生覆盖层 / 标注绘制 |
| `crates/lisnap-ui` | egui/eframe 设置页及开发诊断界面 |
| `docs/architecture.md` | 架构边界、坐标处理与捕获协议说明 |
| `scripts/` | 离线构建和 Wayland 宿主验证脚本 |

## 后续计划

优先完善 **Hyprland 实机验收、多屏与不同缩放场景、剪贴板及输入法兼容性**；后续再考虑全局快捷键、托盘、跨屏选区以及 X11 / Windows / macOS 原生实现。

欢迎通过 [GitHub Issues](https://github.com/MRsoymilk/LiSnap/issues) 反馈问题或建议。架构细节见 [docs/architecture.md](docs/architecture.md)。
