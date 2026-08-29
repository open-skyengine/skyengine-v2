# SkyEngine v2

SkyEngine v2 是按照 [`docs/design.md`](docs/design.md) 设计并使用 Rust 实现的 MRP 应用运行时。
当前基础版本能够安全解析 MRP 容器，加载 V50 和 V80 预编译 MR chunk，在内置寄存器
虚拟机中执行，并通过无头 RGB565 帧缓冲或 SDL2 渲染 MR 绘图调用。

默认字体为 `mythroad/system/gb16.uc2`，其路径相对于 `--work-dir` 解析。仓库内的
`dsm_gm.mrp` 测试样本会经过真实的 `start.mr` 加载链并渲染应用列表界面；运行时
不存在按包名分派或针对特定样本的绘图路径。

`--work-dir` 表示设备文件系统根目录。已安装应用和共享运行时资源沿用设备上的目录布局：

```text
<work-dir>/
  mythroad/
    app.mrp
    system/gb16.uc2
    plugins/*.mrp
```

## 环境要求

- 当前稳定版 Rust 工具链及 Cargo
- SDL2 开发库（Debian/Ubuntu 上为 `libsdl2-dev`）

## 构建与测试

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 发布

GitHub Actions 会在每次分支推送后执行检查，并构建 Linux x86_64 和 Windows x86_64
软件包。只有以下引用会触发发布：

- 每次推送到 `main` 都会更新 `continuous` 预发布版本及可移动的 `continuous` 标签。
- 每个推送的 `v*` 标签（例如 `v0.1.0`）都会创建一个非预发布的版本化 GitHub Release。

各平台软件包包含 `skyengine` 命令行程序、C ABI 动态库、公共头文件、README、MIT
许可证和 `VERSION` 文件。Release 还会包含两个压缩包的 `SHA256SUMS`。

## Flutter 与 C ABI

`skyengine-ffi` 以 C 兼容库的形式向 Flutter 和其他原生宿主公开运行时。Cargo 会生成
`libskyengine.so`、`skyengine.dll` 或 `libskyengine.dylib`；根目录的 `CMakeLists.txt`
还提供真实的 `skyengine-shared` 共享库目标，供 Android Gradle 和桌面 Flutter 构建正常打包。

桥接层在每个进程中维护一个运行时。`skyengine_api_start` 加载应用包并启动原生工作线程；
按键和指针调用将显示事件加入队列，定时器在该工作线程上执行，每个完成呈现的 RGB565 帧
都会复制到稳定的 RGBA 快照中供 `dart:ffi` 读取。`skyengine_api_destroy` 会唤醒并等待
工作线程结束，然后释放运行时状态。完整 ABI 见 [`include/skyengine.h`](include/skyengine.h)。

直接使用 Cargo 构建宿主共享库：

```bash
cargo build --release -p skyengine-ffi
```

也可以从父级 CMake 项目引用共享库目标：

```cmake
set(SKYENGINE_BUILD_SHARED_ONLY ON CACHE BOOL "" FORCE)
add_subdirectory(path/to/skyengine build/skyengine)
```

Android 交叉编译要求安装与目标 ABI 对应的 Rust 标准库目标：

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi
```

桥接层提供渲染、生命周期、定时器、键盘、指针、平台文本编辑器和音频接口。MIDI 和 MP3
会被解码为 44.1 kHz、双声道、交错排列的 S16LE PCM。原生宿主通过
`skyengine_api_audio_render_s16le` 拉取音频帧，并通过 `skyengine_api_audio_is_active`
判断播放是否仍在进行。SDL 前端会自动打开匹配格式的播放设备；无头运行使用静默音频后端，
以保证 E2E 时序确定。Flutter 宿主可轮询 `skyengine_api_motion_active`；返回 `1` 时通过
`skyengine_api_motion` 投递有符号三轴倾斜样本。运行时按 motion 事件 ABI 把三轴结构传给
native MRP。振动输出仍未接入宿主服务，`skyengine_api_take_shake` 保留为兼容接口。

桌面运行可用 `--sound-font FILE.sf2`（或 `SKYENGINE_SOUNDFONT`）选择 GM SoundFont，
由 `rustysynth` 处理 MIDI 序列、乐器采样、鼓组、控制器和效果。未配置 SF2 时继续使用
内置的无采样兼容合成器。C ABI 宿主可在 `skyengine_api_init` 之后、
`skyengine_api_start` 之前调用 `skyengine_api_set_sound_font`；相对路径从 work directory
解析。SoundFont 文件上限为 128 MiB。

在没有窗口服务器的环境中测试 SDL 渲染器：

```bash
SDL_VIDEODRIVER=dummy cargo test -p skyengine-sdl
```

E2E 测试默认使用 SDL 的 `dummy` 驱动。检查或开发测试流程时，如需在可见 SDL 窗口中
同步显示捕获的帧缓冲，可以显式选择真实视频驱动：

```bash
SDL_VIDEODRIVER=x11 pnpm vitest run test/e2e/geyaxz/boot-to-home.test.ts
```

点击预览窗口会输出逻辑屏幕坐标 `[skyengine-sdl] click x=... y=...`，可直接用于
E2E 输入步骤。

## 检查 MRP 包

```bash
cargo run -p skyengine -- inspect test/fixtures/dsm_gm.mrp
cargo run -p skyengine -- inspect --json test/fixtures/dsm_gm.mrp
```

`inspect` 会在不执行 guest 代码的情况下验证并报告容器信息。

## 无头运行

```bash
cargo run -p skyengine -- run \
  --headless \
  --work-dir test/fixtures \
  --frame-output skyengine-frame.ppm \
  test/fixtures/mythroad/dsm_gm.mrp
```

输出文件是二进制 P6 PPM 图像。指定 `--frame-output` 会隐式启用无头模式。常用覆盖参数包括
`--entry NAME`、`--work-dir DIR`、`--font FILE`、`--screen WIDTHxHEIGHT`、
`--dns-map 'HOST->IPv4[:PORT];...'` 和 `--device-date YYYY-M-D|host`。guest 路径
`C:/` 映射到 `--work-dir`，因此 `C:/mythroad/...` 对应工作目录下的 `mythroad/...`。
默认情况下，`rop.skymobiapp.com`、`spd.skymobiapp.com` 和 `wap.skmeg.com` 会映射到
`159.75.119.124`。发往旧版 WAP 网关 `10.0.0.172` 的连接由进程内 HTTP/CONNECT
代理处理。传入 `--dns-map` 会替换默认主机名映射；显式映射 `10.0.0.172` 则会覆盖
进程内代理。

## 使用 SDL2 运行

```bash
cargo run -p skyengine -- run \
  --work-dir test/fixtures \
  test/fixtures/mythroad/dsm_gm.mrp
```

SDL 窗口使用 2 倍逻辑缩放。方向键映射到 MR 方向键，Enter 或 Space 映射到确认键，
`F1` 和 `F2` 映射到左右软键，Escape 映射到返回键。

## 当前实现范围

当前基础版本包括容器读取器、预编译 MR chunk 前端、核心 MR VM、测试样本所需的标准库和
平台调用、RGB565 位图/精灵/文本绘制、MIDI/MP3 播放、安全工作目录枚举及确定性的无头输出。
文本 MR 编译、ARM/Thumb EXT 执行、其余文件/网络服务和 `skydbg` 传输仍属于后续设计里程碑。
遇到不支持的格式或平台操作时，运行时会明确返回失败。

## 许可证

SkyEngine v2 中由本项目自行创作的代码和文档使用 [MIT License](LICENSE) 发布。分发源码
或二进制时，应同时保留 `LICENSE` 中的版权声明和许可声明。第三方依赖及测试材料不因存放在
本仓库中而自动适用 MIT License，它们分别遵循其各自的许可证和权利要求。
