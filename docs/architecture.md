# LiSnap 架构

## 模块边界

`apps/lisnap` 负责解析参数、选择平台候选和启动界面；不直接访问图形驱动。

`lisnap-core` 定义图像、显示器、截图请求、错误、后端 trait 和选区算法；不依赖 egui、窗口系统、异步运行时或操作系统接口。

`lisnap-platform` 实现后端工厂。Linux、Windows、macOS 模块通过 `cfg(target_os)` 隔离，原生依赖只加入相应的 target-specific Cargo 依赖表。Linux/Wayland 已实现真实输出枚举与 SHM 单帧捕获，优先 `ext-image-copy-capture`、回退 `wlr-screencopy`；X11、Windows、macOS 当前仍使用 `StubBackend` 并明确返回 `NotImplemented`。

`lisnap-ui` 使用 egui/eframe + wgpu 绘制普通桌面窗口；它不是截图后端，也不是已经实现的全屏选择覆盖层。此层不决定截图协议。

```text
apps/lisnap ──→ lisnap-platform ──→ lisnap-core
          └──→ lisnap-ui ────────→ lisnap-core
```

## 原始图像与选区

`RgbaImage` 保存经过校验、紧密排列、已消除旋转/镜像的 RGBA8 数据，使用 `Arc<[u8]>` 共享只读像素。原始图像不提供可变访问；绘制遮罩、边框和标注应使用独立的视图状态。

当前是 CPU 图像模型，尚未实现 GPU 图像存储、DMA-BUF 导入或零拷贝。后续引入 GPU 存储时要明确所有权、同步、格式和读回流程，不把共享文件描述符等同于零拷贝保证。

每个输出分别保存物理像素尺寸和可选的逻辑位置/缩放。没有可靠的全局布局时，`logical_geometry` 为 `None`，不猜测显示器排列。原生后端必须先处理行步长、通道顺序和输出旋转；HDR/色彩管理尚未支持。

`selection_to_pixels` 接收同一坐标空间中的选区与输出矩形，规范化反向拖动，裁到单个输出，按照实际物理图像尺寸分别换算 X/Y，最后向外取整。它覆盖负坐标、分数缩放、跨屏交集、无交集和非法数值；它不负责发现屏幕布局或接收鼠标事件。

`CaptureFrame` 的时间戳属于单个输出，不能把多个输出的结果宣称为同一时刻的原子截图。未来服务层也需检查成功结果非空、屏幕描述与图像尺寸一致。

## 计划中的捕获流程

先完成所有目标输出的捕获，再显示选区覆盖层，避免把自身 UI 截进去。接着在静态图像上交互，确认后从原始像素裁剪，再合成用户主动添加的标注。关闭/取消时释放连接、帧和覆盖层资源。

实际捕获不得阻塞 UI 重绘回调。原生 API 的线程约束由对应后端或服务层负责；当前 trait 不强制 `Send`，不提前假定 macOS、Wayland 与 Windows 的线程模型相同。桌面壳层通过 `CaptureLauncher` 启动后台 worker，并在线程内部创建 native backend，因此不跨线程移动后端对象；UI 通过 channel 轮询结果、显示忙碌/错误/帧摘要，并用 `CaptureCancellation` 协作取消 Wayland 等待。

## 原生后端开发顺序

首先面向 Hyprland，探测 Wayland registry 中实际公布的协议。当前实现优先使用 `ext-image-copy-capture` 与相应捕获源协议，兼容 `wlr-screencopy`；没有对应能力就明确返回不支持，不暗中回退到 Portal 或 XWayland。

SHM 单帧捕获已完成：两条路径均支持 ARGB/XRGB、stride、输出方向归一化、光标选项、超时与取消；ext 还会处理动态 buffer constraints 并重试一次。DMA-BUF、HDR/色彩管理仍属于后续优化。系统或合成器的授权要求必须正常处理；本项目不通过 root、CAP_SYS_ADMIN 或 DRM master 抢占作为常规截图路径。

无 Portal 的 Wayland 路径只面向提供所需能力的合成器，不承诺覆盖所有 Wayland 桌面。覆盖层也要单独进行平台适配；Hyprland 的 layer-shell 路径应与普通 egui 窗口分离，不能认为普通全屏窗口天然具备所有置顶、定位和输入行为。

下一阶段在真实 Wayland 会话完成像素验收后实现静态覆盖层与选区；随后再接入 X11 SHM/GetImage、Windows Graphics Capture、macOS ScreenCaptureKit。后三者仍是开发目标，不是已经实现的能力。

## 验证边界

核心测试只验证几何、像素和工厂契约，不读取桌面。`scripts/check-core.sh` 是 Unix 上的无依赖验证入口，并额外编译无 GUI 的 CLI；不能据此声称 GUI 或所有操作系统已经通过测试。

平台真机测试至少需要覆盖：协议缺失、权限拒绝、取消/超时、拔插显示器、不同缩放/负坐标/旋转、多 GPU、捕获光标以及截图前隐藏自身窗口。多屏选区合成与不同像素密度的导出策略应在实现前另行确定。

## 界面 API 参考

- eframe 0.36.2：<https://docs.rs/eframe/0.36.2/eframe/>
- App 接口：<https://docs.rs/eframe/0.36.2/eframe/trait.App.html>
- NativeOptions：<https://docs.rs/eframe/0.36.2/eframe/struct.NativeOptions.html>
