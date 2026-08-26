# Mrpeditor.exe MIDI 行为分析

分析对象为 `build/Mrpeditor.exe`，SHA-256：
`037f6889de5befe457d3df9868325912180078fad2b9c3fda55b67a3737dc959`。它是 2010 年构建的
32 位 .NET Windows Forms 程序。

## 结论

该程序不解析 Standard MIDI File 事件，也没有内置合成器或乐器采样。它先把 MRP 中的
资源解压到临时 `.mid` 文件，再通过 `winmm.dll!mciSendStringA` 交给 Windows MCI MIDI
sequencer。最终音色来自运行该程序的 Windows MIDI 输出设备；通常是 Microsoft GS
Wavetable Synth 及其 GM/DLS 音色库。

因此，复现该程序的关键不是仿造几种 oscillator 波形，而是同时提供完整 MIDI sequencer
和 GM 音源。同一个 MIDI 即使事件时序一致，换用不同 SoundFont 也会有不同音色、包络、
响度和效果。

## 播放调用链

反汇编中 `Mrp编辑助手.Edit_Mrp::btPlay_Click` 的行为如下：

1. 读取当前列表项 `Tag` 中的 MRP 资源字节。
2. 发送 `close music`，关闭前一个 MCI alias。
3. 调用程序自己的 `Mrp解压(byte[], path)`，输出到 `%TEMP%\mrp.mid`。
4. 文件存在时发送 `open %TEMP%\mrp.mid alias music`。
5. 发送 `play music`，不附带 `repeat`、`from`、`to` 或回调参数。

`btStop_Click`、窗口关闭以及选择其他资源时都会发送 `close music`。右键列表项时，程序
只用不区分大小写的最后四个字符 `.mid` 判断是否显示播放控件。所有 MCI 返回码都被丢弃，
程序本身不查询播放位置、时长或设备状态，也没有在这个调用链里设置 MIDI 音量。

## SkyEngine 对应实现

SkyEngine 必须继续向 SDL/C ABI 输出确定格式的 PCM，不能直接依赖仅 Windows 可用的 MCI。
本项目因此采用以下映射：

- `rustysynth` 负责 SMF sequencing、GM program/鼓组、SoundFont 包络、控制器、pitch bend、
  reverb 和 chorus；
- SF2 由宿主显式提供，因为原程序使用的 Windows GM/DLS bank 不在 exe 中，也不能从该
  exe 提取；
- `AudioPlayer` 仍维护单曲替换、停止、循环和 `0..5` 音量，并输出 44.1 kHz 双声道 S16LE；
- 保留 10 分钟音频、100 万 MIDI 事件、128 voices 和 128 MiB SoundFont 上限；
- 未提供 SoundFont 时保留内置无采样合成器，避免现有宿主和自动化环境失去 MIDI 输出。

CLI 使用 `--sound-font FILE.sf2`，也可设置 `SKYENGINE_SOUNDFONT`。C ABI 使用
`skyengine_api_set_sound_font`，调用顺序为 `init -> set_work_dir/set_sound_font -> start`。
