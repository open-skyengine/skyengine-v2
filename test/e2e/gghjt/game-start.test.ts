import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("gghjt 开始游戏", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  const memCheckTime = 15_000

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏正式开始 - 不花屏", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    if (!fs.existsSync(ws.path('mythroad/plugins/netpay.mrp'))) {
      fs.cpSync('test/fixtures/plugins/netpay.mrp', ws.path('mythroad/plugins/netpay.mrp'));
    }
    fs.rmSync(ws.path('mythroad/gghjt'), { force: true, recursive: true });
    fs.rmSync(ws.path('mythroad/cache'), { force: true, recursive: true });
    fs.cpSync('test/fixtures/gghjt', ws.path('mythroad/gghjt'), { recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", { workDir: ws.dir });

    {
      // 检测内存
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(1_000);

    }
    await engine.delay(memCheckTime);
    const boot = await engine.screen("bgm-select");
    // rgb(72,88,0)
    expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);

    // 是否开启音乐？-> 否
    await engine.click(230, 308, 1_000);
    await engine.delay(1_000);

    // 跳过启动剧情
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(152, 112, 32)
    expect(wake.pixel(110, 27)).toEqual([152, 112, 32]);
    {
      // 切换菜单
      await engine.click(162, 291, 3_000);
      await engine.delay(1_000);
      const screen = await engine.screen("menu-continue");
      // rgb(232, 196, 104)
      expect(screen.pixel(162, 291)).toEqual([232, 196, 104 ]);
    }
    {
      // 点击继续游戏，进入付费界面
      await engine.click(116, 291, 3_000);
      await engine.delay(1_000);
      const pay = await engine.screen("pay-start");
      // rgb(104, 104, 224)
      expect(pay.pixel(104, 147)).toEqual([104, 104, 224]);
    }

    {
      // 点击确定付费，进入游戏界面
      await engine.key('LEFT_SOFT', 3_000);
      await engine.delay(4_000);
      const screen = await engine.screen("game-start");
      // rgb(0, 0, 0)
      expect(screen.pixel(88, 44)).toEqual([0, 0, 0]);
    }

    {
      // 两下回车，对话确认后，渲染场景
      await engine.key('ENTER', 3_000);
      await engine.delay(1_000);
      await engine.key('ENTER', 3_000);
      await engine.delay(8_000);
      const screen = await engine.screen("scene-render");
      // rgb(192, 148, 96)
      expect(screen.pixel(55, 302)).toEqual([192, 148, 96]);
      // 楼房背景由离屏 cache 合成后复制到主屏；这些点曾是深色条纹坏色。
      expect(screen.pixel(92, 95)).toEqual([72, 60, 72]);
      expect(screen.pixel(108, 95)).toEqual([128, 128, 112]);
      expect(screen.pixel(124, 95)).toEqual([128, 128, 112]);
      expect(screen.pixel(172, 95)).toEqual([208, 200, 136]);
    }

  });
  it("游戏直接开始 - 不缺渲染", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    if (!fs.existsSync(ws.path('mythroad/plugins/netpay.mrp'))) {
      fs.cpSync('test/fixtures/plugins/netpay.mrp', ws.path('mythroad/plugins/netpay.mrp'));
    }
    fs.rmSync(ws.path('mythroad/cache'), { force: true, recursive: true });
    fs.rmSync(ws.path('mythroad/gghjt'), { force: true, recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", { workDir: ws.dir });

    {
      // 检测内存
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(1_000);

    }
    await engine.delay(memCheckTime);
    const boot = await engine.screen("bgm-select");
    // rgb(72,88,0)
    expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);

    // 是否开启音乐？-> 否
    await engine.click(230, 308, 1_000);
    await engine.delay(1_000);

    // 跳过启动剧情
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(152, 112, 32)
    expect(wake.pixel(110, 27)).toEqual([152, 112, 32]);
    {
      // 点击开始游戏，进入游戏界面
      await engine.click(116, 291, 3_000);
      await engine.delay(6_000);
      // rgb(248, 252, 0)
      await engine.waitForPixel(42, 140, [248, 252, 0], { name: "game-start", timeoutMs: 6_000 });
    }

    {
      // 六次回车，对话确认后，渲染场景
      for (let i=0; i<6; i++) {
        await engine.key('ENTER', 3_000);
        await engine.delay(1_000);
      }
      const screen = await engine.screen("scene-render");
      // rgb(192, 148, 96)
      expect(screen.pixel(55, 302)).toEqual([192, 148, 96]);
      // 火车顶部场景；这些点覆盖离屏背景和底部栏，防止只显示中间视口。
      expect(screen.pixel(120, 20)).toEqual([8, 8, 0]);
      expect(screen.pixel(120, 300)).toEqual([192, 148, 96]);
    }

  });
});
