import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("optwar", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("临时测试", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    if (!fs.existsSync(ws.path('mythroad/plugins/netpay.mrp'))) {
      fs.cpSync('test/fixtures/plugins/netpay.mrp', ws.path('mythroad/plugins/netpay.mrp'));
    }
    // 本用例验证前台 advbar 插件与游戏主画面的屏幕合成，运行前显式准备插件资源。
    if (!fs.existsSync(ws.path('mythroad/plugins/advbar.mrp'))) {
      fs.cpSync('test/fixtures/plugins/advbar.mrp', ws.path('mythroad/plugins/advbar.mrp'));
    }
    engine = await SkyEngineE2e.start("test/fixtures/optwar.mrp", { workDir: ws.dir });

    await engine.delay(2000);
    const boot = await engine.screen("bgm-select");
    expect(boot.pixel(150, 308)).toEqual([0, 0, 0]);
    // rgb(248, 0, 0)
    expect(boot.pixel(227, 301)).toEqual([248, 0, 0]);

    // 是否开启音乐？-> 否
    await engine.click(227, 301, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(128, 48, 40)
    expect(wake.pixel(110, 27)).toEqual([128, 48, 40]);
    expect(wake.pixel(120, 20)).toEqual([176, 120, 120]);
    // rgb(24, 24, 24)
    expect(wake.pixel(83, 267)).toEqual([24, 24, 24]);
    // rgb(0, 252, 0)
    expect(wake.pixel(98, 264)).toEqual([0, 252, 0]);

    {
      // 第一次方向键先被前台 advbar 关闭流程消费，只应让顶部广告条消失。
      await engine.key('RIGHT', 1_000)
      await engine.delay(1_000);
      const afterRight = await engine.screen("after-right");
      expect(afterRight.pixel(110, 27)).not.toEqual([128, 48, 40]);
      // rgb(0, 252, 0)
      expect(afterRight.pixel(98, 264)).toEqual([0, 252, 0]);
      // 开始游玩
      await engine.key('ENTER', 1_000)
      await engine.delay(1_000);
      const afterSecondRight = await engine.screen("after-second-right");
      expect(afterSecondRight.pixel(98, 264)).not.toEqual(afterRight.pixel(98, 264));
    }
    {
      // 跳过介绍
      await engine.delay(1_000);
      await engine.key('ENTER', 1_000)
      await engine.delay(1_000);
      await engine.key('ENTER', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('start-scene')
        // rgb(200, 252, 248)
        expect(screen.pixel(22, 314)).toEqual([200, 252, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    await engine.delay(60_000);
  });
});
