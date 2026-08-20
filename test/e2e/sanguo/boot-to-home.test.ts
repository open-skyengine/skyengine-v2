import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

// 490111 幻想三国(240x320,Lua FULL 启动 + cfunction.ext wrapper + game.ext 私有加载)。
// fixture 来源:build/mythroad/490111_240x320_sanguo.mrp,SHA-256
// bcbac32ad7567e1112f0c15db6c8aae9752bfe4faf2f2492f5665e7332c51702。
//
// 回归目标:wrapper 会把 0x40000000 映射带的外部 arena 临时接入 LG_mem offset
// 空闲链(直接改写 table[108]/[110]/[146] 指向的共享变量)。宿主 table[0]/[1]/[2]
// 必须每次从这些 ARM 可见 slot 读取当前 base/end/head;若沿用初始化时缓存的
// 固定边界,wrapper teardown 会在首个同步 code-0 调用内活锁,启动黑屏且进程
// 只能强杀(见 docs/修复记录/sanguo-black-screen-*.md)。
describe("sanguo 启动", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏正常启动并处理确认键", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/sanguo_490111.mrp", { workDir: ws.dir });

    // 游戏启动后按 IP 直连已停服的 211.155.236.226:20000,失败后自绘"连接超时"
    // 提示框(蓝色标题栏"提示")。该帧要求 code-0 已返回、timer 驱动的网络状态机
    // 在事件循环中推进,活锁回归时永远等不到。超时放宽到 180s 以覆盖真实网络下
    // TCP 对无响应地址的连接超时;无路由环境(CI 沙箱)在数秒内即出现。
    const dialog = await engine.waitForPixel(120, 8, [48, 180, 232], {
      name: "dialog",
      timeoutMs: 180_000,
      intervalMs: 2_000
    });
    // 底部"确定"软键栏蓝色
    expect(dialog.pixel(120, 310)).toEqual([8, 140, 216]);
    // 正文"连接超时,请检查网络状况..."黑色文字区
    expect(dialog.pixel(200, 45)).toEqual([8, 4, 0]);

    // 首帧(draw 1)是游戏在同步 code-0 期间自绘的"幻想三国"加载图,证明启动
    // 画面本身正常而非仅有提示框。draw 1..3 只有加载进度条区域(y 268..273)
    // 不同,断言点避开该区域,可从保留帧回读、不依赖采样时机。
    const splash = await engine.screenDraw(1, "splash");
    // 标题"幻"字金黄描边
    expect(splash.pixel(30, 22)).toEqual([248, 240, 96]);
    // 人物立绘橙色高光
    expect(splash.pixel(60, 60)).toEqual([232, 128, 64]);
    // "游戏加载中..."文本框暖色底纹
    expect(splash.pixel(200, 195)).toEqual([248, 176, 136]);
    // 文本框下沿深色描边
    expect(splash.pixel(120, 225)).toEqual([32, 4, 0]);

    // 提示框文案为"点击确定退出游戏":ENTER 走按键分发→游戏主动退出路径,
    // 证明事件处理链路完好且进程能干净退出(活锁回归时只能 SIGKILL)。
    // 服务器停运后这是该画面唯一有效的交互。
    await engine.key("ENTER", 8_000);
    expect(await engine.waitForExit(8_000)).toBe(true);
  }, 240_000);
});
