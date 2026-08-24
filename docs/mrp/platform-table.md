# Mythroad 平台函数表

原生 EXT 通过一个按槽位编号组织的 32 位表访问运行时能力。槽位顺序是 ABI：即便宿主内部 API 完全不同，暴露给 guest 的索引和数据种类也必须匹配目标 SDK。

本页记录的是一套广泛部署、共 150 槽的表。它不代表所有 Mythroad 版本；较早或较晚 SDK 可能在保留位、尾部扩展和少数数据项上不同。加载器应根据已知 SDK 特征验证表长，并为不支持的槽提供确定的失败行为。

“种类”列含义：

- **函数**：槽值是 guest 可调用地址，调用时需处理 ARM/Thumb 状态。
- **数据**：槽值是变量、缓冲区、数组或字符串的 guest 地址；不能直接跳转调用。
- **子表**：槽值指向另一张 ABI 表。
- **保留**：观察版本中应为零或无可用接口。

“版本”列的“基础”表示该套 150 槽表中的核心项，“常见”表示广泛出现但能力可由平台返回不支持，“变体”表示名称、数据内容或功能在 SDK 间尤其容易变化。

## C 运行库与内存（槽 0-21）

| 槽 | 种类 | ABI 名称 | 作用 | 版本 |
|---:|---|---|---|---|
| 0 | 函数 | `mr_malloc` | 分配应用内存 | 基础 |
| 1 | 函数 | `mr_free` | 释放应用内存；历史签名可带长度 | 基础 |
| 2 | 函数 | `mr_realloc` | 调整应用内存；历史签名可带旧长度 | 基础 |
| 3 | 函数 | `memcpy` | 复制不重叠内存 | 基础 |
| 4 | 函数 | `memmove` | 复制可重叠内存 | 基础 |
| 5 | 函数 | `strcpy` | 复制 C 字符串 | 基础 |
| 6 | 函数 | `strncpy` | 限长复制字符串 | 基础 |
| 7 | 函数 | `strcat` | 拼接字符串 | 基础 |
| 8 | 函数 | `strncat` | 限长拼接字符串 | 基础 |
| 9 | 函数 | `memcmp` | 比较内存 | 基础 |
| 10 | 函数 | `strcmp` | 比较字符串 | 基础 |
| 11 | 函数 | `strncmp` | 限长比较字符串 | 基础 |
| 12 | 函数 | `strcoll` | 按平台规则比较字符串 | 常见 |
| 13 | 函数 | `memchr` | 在内存中查找字节 | 基础 |
| 14 | 函数 | `memset` | 填充内存 | 基础 |
| 15 | 函数 | `strlen` | 计算 C 字符串长度 | 基础 |
| 16 | 函数 | `strstr` | 查找子字符串 | 基础 |
| 17 | 函数 | `sprintf` | 格式化字符串 | 基础 |
| 18 | 函数 | `atoi` | 十进制字符串转整数 | 基础 |
| 19 | 函数 | `strtoul` | 按进制转无符号长整数 | 基础 |
| 20 | 函数 | `rand` | 取得伪随机数 | 基础 |
| 21 | 保留 | - | 观察表中为空 | 保留 |

这些接口使用 guest 地址和 32 位宽度。宿主必须检查字符串终止符和缓冲区范围，不能直接把 guest 参数传给宿主 libc。
槽 17 的本地验证子集还包括 SDK 的裸 `%m`：它输出单字节 `m` 且不消费变参；带
flag、宽度、精度或长度修饰的 `%m` 没有 ABI 证据，因此保持 Unsupported。

## 模块注册与基础平台能力（槽 22-39）

| 槽 | 种类 | ABI 名称 | 作用 | 版本 |
|---:|---|---|---|---|
| 22 | 函数 | `mr_stop_ex` | 停止当前应用/扩展执行 | 常见 |
| 23 | 子表 | internal table | 指向内部服务表 | 变体 |
| 24 | 子表 | port table | 指向移植层服务表 | 变体 |
| 25 | 函数 | `_mr_c_function_new` | 登记 native helper 和模块参数 | 基础 |
| 26 | 函数 | `mr_printf` | 输出诊断信息 | 基础 |
| 27 | 函数 | `mr_mem_get` | 向平台取得一块应用内存 | 基础 |
| 28 | 函数 | `mr_mem_free` | 将整块应用内存归还平台 | 基础 |
| 29 | 函数 | `mr_drawBitmap` | 绘制基础位图 | 基础 |
| 30 | 函数 | `mr_getCharBitmap` | 取得字符字模 | 基础 |
| 31 | 函数 | `mr_timerStart` | 启动平台计时器 | 基础 |
| 32 | 函数 | `mr_timerStop` | 停止平台计时器 | 基础 |
| 33 | 函数 | `mr_getTime` | 取得单调/系统计时值 | 基础 |
| 34 | 函数 | `mr_getDatetime` | 取得日期时间 | 基础 |
| 35 | 函数 | `mr_getUserInfo` | 取得设备/用户信息 | 常见 |
| 36 | 函数 | `mr_sleep` | 请求休眠指定毫秒 | 常见 |
| 37 | 函数 | `mr_plat` | 数值型通用平台命令 | 变体 |
| 38 | 函数 | `mr_platEx` | 缓冲区型扩展平台命令 | 变体 |
| 39 | 函数 | `mr_ferrno` | 取得最近文件错误 | 基础 |

槽 23 和 24 是表地址，不是可调用函数。`mr_plat`/`mr_platEx` 的命令号空间属于平台扩展，只有已验证的命令才能实现为兼容行为。

基线 headless profile 的 `mr_getUserInfo` 向非空输出指针写入确定性的 64 字节
设备/用户信息记录并返回 `0`；空输出指针返回 `-1`。该基础 ABI 与可选的 MTK
native extension 固定内存窗口相互独立。

基线 profile 已验证的 `mr_plat` 命令包括：

- `101` 的参数 `3` 选择横向屏幕模式，参数 `0` 恢复纵向模式，成功返回 `0`。模式
  切换交换宽高，同时更新槽 92/93 和屏幕 bitmap 描述符；屏幕缓冲区地址与容量不变，
  宿主显示尺寸在下一次提交前同步更新。其他模式值不属于已验证基线；
- `1011` 的参数 `0..1` 通知网络/支付 helper 进入前台操作及其后续状态，headless
  profile 接受这些状态通知并返回 `0`；
- `1206(0)` 初始化确定性的 motion provider，`4002(0)` 查询该 provider，
  `4005(2)` 选择已验证的事件模式；headless 后端接受这些操作但不产生传感器样本；
- `1101` 的参数 `2` 查询可选设备指标。确定性 headless profile 没有对应提供者，返回
  `-1`；观察到的 EXT 调用方把该值归一为中性的 `0`。
- `1302` 的参数 `0..5` 设置多媒体音量级别，成功返回 `0`。无音频输出的
  headless 后端仍接受该状态设置，但不会产生宿主声音。
- `2703(0)` 是本地 fixture 验证的无参数平台通知。包装器只把返回值归一为 `0/-1`，
  两个调用点随后都覆盖该值；headless profile 接受该通知并返回 `0`。其他参数保持
  Unsupported。

基线 profile 只白名单以下已由本地 fixture 验证的 `mr_platEx` 子集：

- `1204` 接受单字节逻辑卷 `C`、`X`、`Y` 或 `Z`，并把这些应用逻辑卷解析到当前
  物理应用卷 `C:/mythroad/`；不支持的逻辑卷返回 `-1`。文件 API 中显式出现的
  `X:`、`Y:`、`Z:` 路径仍分别映射到工作区的 `disk/x`、`disk/y`、`disk/z`，不与
  该逻辑卷解析命令混用。
- `1305` 接受逻辑卷 `C`、`X`、`Y` 或 `Z` 的单字节、NUL 结尾或冒号形式，返回
  16 字节确定性虚拟盘几何信息：总容量 256 MiB、可用容量 128 MiB。调用方仍负责
  释放平台返回的只读结构；不支持的卷返回 `-1`。
- `3001` 接受 12 字节 `{JPEG 地址, JPEG 长度, 1}`，同步返回平台内部的最小 8 字节
  `{u32 宽度, u32 高度}` 元数据；该白名单只承诺调用方实际读取的两个字段，不推断
  可能存在的后续字段。源缓冲区仍由调用方持有，无效或超限的 JPEG 返回 `-1` 并
  清空输出字段。
- `3002` 接受 24 字节 `{JPEG 地址, JPEG 长度, 宽度, 高度, 1, 目标地址}`，把
  解码结果写入调用方提供的 little-endian RGB565 缓冲区。尺寸必须与 JPEG 元数据
  完全一致；命令不返回平台所有的图像缓冲区。
- `2013` 只白名单输入、输出和回调均为空的参数形式。调用方在返回后无条件覆盖
  返回寄存器；基线 profile 不建立未经证实的平台状态，并返回统一的不可用值 `-1`。
- `2023` 只接受最长 4096 字节、不含 NUL、以 `.mp3` 结尾且可解析为非空文件的
  caller-owned 路径，输出和回调必须为空。资源可以是工作区文件，也可以是当前包中
  与路径 basename 精确同名的条目；有输出能力的宿主解码并播放该资源，headless
  profile 通过静默音频 sink 消费请求。文件不存在或音频无法解码时返回 `-1`，其他
  未经验证的形态保持 Unsupported。
- `2043` 的本地调用紧跟已验证的 `2023` 请求；只白名单全空参数形式，停止当前音轨
  并返回 `0`。任何输入、输出或回调非空的形态保持 Unsupported。
- `2093` 只白名单全空参数的多媒体状态查询；播放中返回 `1001`，空闲返回 `1003`。
  确定性 headless profile 始终为空闲。任何输入、输出或回调非空的形态保持
  Unsupported。
- `2700` 只接受 16 字节 `{WAV 路径地址, 0, 0, 1}` 和全空输出/回调；路径是最长
  4096 字节、以 `.wav` 结尾且不含空路径段的 caller-owned C 字符串。headless
  profile 没有录音 provider，返回统一的不可用值 `-1`，也不创建或写入目标文件。
  其他结构字段、模式和路径形态保持 Unsupported。

`3004`/`3005` 的本地证据只覆盖调用方读取的若干对象偏移和一个释放句柄，无法闭合
完整对象布局、所有权和错误契约，因此不在白名单内并保持 Unsupported。

## 文件、设备与通信（槽 40-62）

| 槽 | 种类 | ABI 名称 | 作用 | 版本 |
|---:|---|---|---|---|
| 40 | 函数 | `mr_open` | 打开文件 | 基础 |
| 41 | 函数 | `mr_close` | 关闭文件句柄 | 基础 |
| 42 | 函数 | `mr_info` | 查询文件或目录信息 | 基础 |
| 43 | 函数 | `mr_write` | 写文件 | 基础 |
| 44 | 函数 | `mr_read` | 读文件 | 基础 |
| 45 | 函数 | `mr_seek` | 改变文件位置 | 基础 |
| 46 | 函数 | `mr_getLen` | 取得文件长度 | 基础 |
| 47 | 函数 | `mr_remove` | 删除文件 | 基础 |
| 48 | 函数 | `mr_rename` | 重命名文件 | 基础 |
| 49 | 函数 | `mr_mkDir` | 创建目录 | 基础 |
| 50 | 函数 | `mr_rmDir` | 删除目录 | 基础 |
| 51 | 函数 | `mr_findStart` | 开始目录枚举 | 基础 |
| 52 | 函数 | `mr_findGetNext` | 取得下一目录项 | 基础 |
| 53 | 函数 | `mr_findStop` | 结束目录枚举 | 基础 |
| 54 | 函数 | `mr_exit` | 请求应用退出 | 基础 |
| 55 | 函数 | `mr_startShake` | 启动振动 | 常见 |
| 56 | 函数 | `mr_stopShake` | 停止振动 | 常见 |
| 57 | 函数 | `mr_playSound` | 播放声音 | 常见 |
| 58 | 函数 | `mr_stopSound` | 停止声音 | 常见 |
| 59 | 函数 | `mr_sendSms` | 发送短信 | 变体 |
| 60 | 函数 | `mr_call` | 发起电话呼叫 | 变体 |
| 61 | 函数 | `mr_getNetworkID` | 取得当前网络标识 | 常见 |
| 62 | 函数 | `mr_connectWAP` | 打开 WAP/网络入口 | 变体 |

文件路径语法、根目录和权限由移植层定义。短信和呼叫等有外部副作用的接口只能由
宿主预置的内部平台策略决定；CLI、嵌入方和测试调用不得按次传入放行、拒绝或结果，
这也不改变 guest 可见的 ABI 槽位。

`mr_info` 返回 `1` 表示文件、`2` 表示目录、`8` 表示路径不存在或文件类型无效；
无法安全解析的 guest 路径返回 `MR_FAILED`。

基线 headless profile 支持 `mr_playSound(type, data, len, looped)` 的内存 MIDI
（类型 `0`）和 MP3（类型 `2`）形式，其中 `data` 和 `len` 必须指向非空的有效
guest 缓冲区，`looped` 为 `0` 或 `1`。
有输出能力的宿主把资源解码为 44.1 kHz 双声道 PCM，并支持单曲循环；headless
profile 使用无输出音频 sink，调用成功推进 guest 状态但不产生宿主声音。
`mr_stopSound` 停止当前音轨，在没有正在播放的声音时也成功返回。

## UI 与网络（槽 63-90）

| 槽 | 种类 | ABI 名称 | 作用 | 版本 |
|---:|---|---|---|---|
| 63 | 函数 | `mr_menuCreate` | 创建菜单 | 常见 |
| 64 | 函数 | `mr_menuSetItem` | 设置菜单项 | 常见 |
| 65 | 函数 | `mr_menuShow` | 显示菜单 | 常见 |
| 66 | 保留 | menu focus (historical) | 观察表中为空，旧版本曾用于菜单焦点 | 变体 |
| 67 | 函数 | `mr_menuRelease` | 释放菜单 | 常见 |
| 68 | 函数 | `mr_menuRefresh` | 刷新菜单 | 常见 |
| 69 | 函数 | `mr_dialogCreate` | 创建对话框 | 常见 |
| 70 | 函数 | `mr_dialogRelease` | 释放对话框 | 常见 |
| 71 | 函数 | `mr_dialogRefresh` | 刷新对话框 | 常见 |
| 72 | 函数 | `mr_textCreate` | 创建文本查看器 | 常见 |
| 73 | 函数 | `mr_textRelease` | 释放文本查看器 | 常见 |
| 74 | 函数 | `mr_textRefresh` | 刷新文本查看器 | 常见 |
| 75 | 函数 | `mr_editCreate` | 创建文本编辑器 | 常见 |
| 76 | 函数 | `mr_editRelease` | 释放文本编辑器 | 常见 |
| 77 | 函数 | `mr_editGetText` | 取得编辑器文本 | 常见 |
| 78 | 函数 | `mr_winCreate` | 创建平台窗口 | 常见 |
| 79 | 函数 | `mr_winRelease` | 释放平台窗口 | 常见 |
| 80 | 函数 | `mr_getScreenInfo` | 取得屏幕属性 | 基础 |
| 81 | 函数 | `mr_initNetwork` | 初始化网络并登记回调 | 常见 |
| 82 | 函数 | `mr_closeNetwork` | 关闭网络服务 | 常见 |
| 83 | 函数 | `mr_getHostByName` | 异步解析主机名 | 常见 |
| 84 | 函数 | `mr_socket` | 创建网络套接字 | 常见 |
| 85 | 函数 | `mr_connect` | 连接远端地址 | 常见 |
| 86 | 函数 | `mr_closeSocket` | 关闭套接字 | 常见 |
| 87 | 函数 | `mr_recv` | 接收流数据 | 常见 |
| 88 | 函数 | `mr_recvfrom` | 接收数据报 | 常见 |
| 89 | 函数 | `mr_send` | 发送流数据 | 常见 |
| 90 | 函数 | `mr_sendto` | 发送数据报 | 常见 |

UI 句柄和 socket 都是 guest ABI 整数，不应与宿主原生句柄直接等同。异步回调必须在登记它的模块仍存活时才可投递。

基线 headless profile 已验证文本查看器类型 `2`。创建后由平台绘制标题、换行正文和
返回软键；返回操作通过 `MR_DIALOG_EVENT` 的 cancel 结果交给 guest，guest 再调用
`mr_textRelease` 关闭查看器并恢复此前画面。刷新只重绘仍位于 UI 栈顶的有效句柄。

`mr_winCreate` 在 headless profile 中创建受模块所有权约束的非零 opaque 句柄，不建立
宿主原生窗口；`mr_winRelease` 只释放同一模块持有的精确句柄。窗口与菜单、对话框和
文本查看器共享 64 个 live UI 句柄上限，应用替换或模块初始化回滚会清理相应句柄。

## 共享运行时数据（槽 91-112）

| 槽 | 种类 | ABI 名称 | 作用 | 版本 |
|---:|---|---|---|---|
| 91 | 数据 | screen buffer pointer variable | 指向“屏幕缓冲区地址变量” | 基础 |
| 92 | 数据 | screen width | 屏幕宽度变量地址 | 基础 |
| 93 | 数据 | screen height | 屏幕高度变量地址 | 基础 |
| 94 | 数据 | screen bit depth | 屏幕位深变量地址 | 基础 |
| 95 | 数据 | bitmap array/base | 位图资源数组或基址 | 基础 |
| 96 | 数据 | tile array/base | tile 资源数组或基址 | 基础 |
| 97 | 数据 | map array/base | 地图资源数组或基址 | 基础 |
| 98 | 数据 | sound array/base | 声音资源数组或基址 | 基础 |
| 99 | 数据 | sprite array/base | 精灵资源数组或基址 | 基础 |
| 100 | 数据 | current package filename | 当前 MRP 文件名字符串 | 基础 |
| 101 | 数据 | current start filename | 当前入口文件名字符串 | 基础 |
| 102 | 数据 | previous package filename | 前一个 MRP 文件名字符串 | 常见 |
| 103 | 数据 | previous start filename | 前一个入口文件名字符串 | 常见 |
| 104 | 数据 | RAM-backed MRP pointer | 内存 MRP 数据地址变量 | 常见 |
| 105 | 数据 | RAM-backed MRP length | 内存 MRP 长度变量 | 常见 |
| 106 | 数据 | sound-enabled flag | 声音开关变量 | 基础 |
| 107 | 数据 | vibration-enabled flag | 振动开关变量 | 基础 |
| 108 | 数据 | application heap base | 应用堆起点变量 | 基础 |
| 109 | 数据 | application heap length | 应用堆长度变量 | 基础 |
| 110 | 数据 | application heap end | 应用堆末端变量 | 基础 |
| 111 | 数据 | application heap free/left | 应用堆剩余量变量 | 基础 |
| 112 | 数据 | SMS configuration buffer | 短信配置缓冲区 | 变体 |

这些槽通常是“变量的地址”，有时变量内容又是另一个 guest 地址，形成二级间接。应按目标槽的定义读写，不能统一只解引用一次。

## 摘要、持久配置与绘图（槽 113-132）

| 槽 | 种类 | ABI 名称 | 作用 | 版本 |
|---:|---|---|---|---|
| 113 | 函数 | MD5 init | 初始化 MD5 状态 | 常见 |
| 114 | 函数 | MD5 append | 向 MD5 状态追加数据 | 常见 |
| 115 | 函数 | MD5 finish | 输出 MD5 摘要 | 常见 |
| 116 | 函数 | load SMS config | 装载短信配置 | 变体 |
| 117 | 函数 | save SMS config | 保存短信配置 | 变体 |
| 118 | 函数 | display/update rectangle | 将指定矩形更新到显示设备 | 基础 |
| 119 | 函数 | draw point | 绘制点 | 基础 |
| 120 | 函数 | draw bitmap | 绘制位图 | 基础 |
| 121 | 函数 | draw transformed bitmap | 按变换矩阵绘制位图 | 常见 |
| 122 | 函数 | draw rectangle | 绘制矩形 | 基础 |
| 123 | 函数 | draw text | 绘制文本 | 基础 |
| 124 | 函数 | bitmap bounds/check | 位图范围或有效性检查 | 变体 |
| 125 | 函数 | read file from MRP | 从当前包上下文读取文件 | 基础 |
| 126 | 函数 | wide-string length | 计算宽字符串长度 | 基础 |
| 127 | 函数 | register application/package | 登记应用或包信息 | 变体 |
| 128 | 函数 | extended text drawing | 带扩展属性绘制文本 | 常见 |
| 129 | 函数 | graphics effect configuration | 配置图形效果 | 变体 |
| 130 | 函数 | generic command channel | 通用数值命令通道 | 变体 |
| 131 | 函数 | string/buffer command channel | 通用字符串/缓冲命令通道 | 变体 |
| 132 | 函数 | legacy encoding to UCS-2 | 旧编码字符串转换为 UCS-2 | 常见 |

槽 113-115 的状态结构也是 guest 内存，大小和对齐要匹配 SDK 声明。字符绘制和编码转换通常使用 16 位字符单元，但字节序仍应由目标 ABI 决定。

## 算术、解压与尾部扩展（槽 133-149）

| 槽 | 种类 | ABI 名称 | 作用 | 版本 |
|---:|---|---|---|---|
| 133 | 函数 | signed division helper | 有符号整数除法辅助函数 | 基础 |
| 134 | 函数 | signed modulo helper | 有符号整数取模辅助函数 | 基础 |
| 135 | 数据 | minimum/free-memory statistic | 最低剩余内存等统计变量 | 变体 |
| 136 | 数据 | peak/top-memory statistic | 峰值/顶部内存统计变量 | 变体 |
| 137 | 函数 | CRC update | 更新 CRC 值 | 常见 |
| 138 | 数据 | start-file parameter | 入口文件参数数据 | 变体 |
| 139 | 数据 | SMS return flag | 短信返回标志变量 | 变体 |
| 140 | 数据 | SMS return value | 短信返回值变量 | 变体 |
| 141 | 函数 | decompression helper | 解压缓冲区或流 | 常见 |
| 142 | 数据 | exit callback pointer | 退出回调变量地址 | 变体 |
| 143 | 数据 | exit callback data | 退出回调参数变量地址 | 变体 |
| 144 | 数据 | current entry string | 当前入口字符串 | 变体 |
| 145 | 函数 | platform glyph drawing | 由平台直接绘制字形 | 变体 |
| 146 | 数据 | free-list head / memory extension | 内存空闲链表头；后期 SDK 可能另作扩展 | 变体 |
| 147 | 函数 | transformed bitmap extension | 扩展变换位图绘制 | 变体 |
| 148 | 函数 | region drawing extension | 扩展区域绘制 | 变体 |
| 149 | 保留 | - | 观察表中为空 | 保留 |

除法辅助函数的除零行为必须匹配目标 ARM 工具链约定。槽 142 保存的是回调相关变量，不等于可以无条件直接调用的固定函数入口。

当前 MTK profile 的槽 138 直接指向一个 128 字节的 start-file parameter 块。该块由
平台会话持有，而不属于某个 EXT 实例；成功替换应用时，宿主在执行新 guest 入口前
逐字节迁移其最新内容。内容格式和清除时机由 guest 决定，宿主不得按包名、guest
地址或画面内容解释它，也不得把它用作按次授权。该块必须使用独立 backing，不能与
相邻的四字节数据槽别名。槽 102/103 的前一个应用标识与槽 138 的入口参数彼此独立。

## 实现规则

平台表的实现应遵守以下边界：

1. 建表时显式写出每个槽的种类，避免把数据地址误注册为函数。
2. guest 调用函数槽时验证索引、非零地址、执行权限和 Thumb bit。
3. guest 访问数据槽时验证读写方向、宽度、对齐和二级指针范围。
4. 保留槽保持为零；未支持的可选函数使用 ABI 允许的失败桩，不伪造成功。
5. 子表分别做长度和版本验证，不能因主表有 150 槽就推断子表布局。
6. 项目内部可以拆分或合并服务，但不得让内部函数编号反向污染 guest ABI。

**证据：稳定 SDK ABI（槽序）；版本/平台相关（命令通道、共享数据和尾部扩展的具体语义）。**
