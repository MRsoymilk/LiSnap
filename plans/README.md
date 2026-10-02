# LiSnap 开发阶段

## S0：Rust 项目框架（源码已建立，GUI 构建验收待完成）

- [x] Cargo workspace：应用入口、核心、平台、界面四层。
- [x] 移除旧的 Qt/CMake/C++ 骨架，使用 egui/eframe + wgpu 界面源码。
- [x] 截图 trait、错误、显示器信息、不可变 RGBA8 图像和裁剪。
- [x] 负坐标、反向选区、分数缩放、边界裁剪和非法数据处理。
- [x] Wayland、X11、Windows、macOS 模块和明确失败的占位实现。
- [x] CLI 参数、后端清单、无 GUI 构建入口。
- [x] 本机离线 19 项测试通过，CLI 编译与版本/清单运行通过。
- [x] 完成 Cargo 依赖解析并生成 Cargo.lock。
- [x] 本机完整 `cargo check --workspace --all-targets` 与 `cargo clippy --workspace --all-targets -- -D warnings`。
- [x] 无显示服务器环境下执行一帧 egui UI 布局 smoke test。
- [x] GUI 真正启动验证：通过显式连接现有 `/run/user/1000/wayland-1`，完整 GUI 二进制在 Wayland 会话中持续运行 5 秒且无启动错误。
- [ ] Windows/macOS 构建验证；本机未安装对应 Rust target，已加入原生 GitHub Actions 构建矩阵，并要求各原生 runner 执行 workspace check、平台后端测试和无 GUI CLI 测试，待远端实际运行后验收。

优先完成 S0 剩余验收，不把源码框架或环境变量推断计为真实截图支持。

## S1：Hyprland / Wayland 单输出捕获

- [ ] 连接 Wayland，枚举输出，探测真实协议能力与可用捕获源。（registry 探测与 `--probe-wayland` 已实现；EndlessVibe bubblewrap 未挂载宿主 Wayland socket，真机会话验收待在桌面环境执行。）
- [x] 按实际能力选择 ext-image-copy-capture 或 wlr-screencopy，无 Portal 回退。已真实绑定 ext capture/source 与 `zwlr_screencopy_manager_v1`，仅在两个 ext manager 均绑定成功时优先 ext，否则回退 wlr。
- [ ] SHM 单帧捕获，处理格式、步长、旋转、权限拒绝和协议缺失。（wlr 路径已实现 memfd + `wl_shm_pool` 单帧复制，支持 ARGB/XRGB、stride、Y-invert 与协议/格式失败；ext 捕获、超时和更细的权限错误仍待继续。）
- [ ] 超时、取消、资源释放、非空结果与图像尺寸一致性验证。
- [ ] 真机验证：获得当前输出的真实像素，不截入自身选择 UI。

## S2：静态覆盖层与选区

- [ ] 单屏冻结视觉效果、拖动框选、Esc 取消。
- [ ] Wayland 覆盖层平台适配，之后扩展多输出与跨屏选区。
- [ ] 原图裁剪、尺寸提示与选区边界测试。

## S3：基础截图工具

- [ ] PNG 导出、复制到剪贴板、快捷键、托盘。
- [ ] 标注数据模型、矩形/箭头/文字、撤销/重做。

## S4：其他平台和优化

- [ ] X11 原生捕获与覆盖层。
- [ ] Windows Graphics Capture 与 Windows 窗口适配。
- [ ] macOS ScreenCaptureKit 与 macOS 窗口/授权适配。
- [ ] DMA-BUF/GPU 存储、颜色/HDR、多 GPU 和性能优化。

每次实现一个可独立验证的小范围，不混入其他项目改动；按实际改动命名提交，不自动 push。不能把离线核心测试写成全平台或 GUI 验证通过。
