import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("opbzqe 进入主菜单", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏准备 - 顶部正常", async () => {
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
    engine = await SkyEngineE2e.start("test/fixtures/opbzqe.mrp", { workDir: ws.dir });

    await vi.waitFor(async () => {
      const boot = await engine!.screen("bgm-select");
      expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);
      // rgb(248, 252, 248)
      expect(boot.pixel(126, 165)).toEqual([248, 252, 248]);
    }, {
      timeout: 30_000,
      interval: 1_000
    })

    // 是否开启音乐？-> 否
    await engine.click(214, 293, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(128, 48, 40)
    expect(wake.pixel(110, 27)).toEqual([128, 48, 40]);
    expect(wake.pixel(120, 20)).toEqual([176, 120, 120]);
    expect(wake.pixel(113, 124)).toEqual([184, 192, 48]);

    {
      // 按一下右方向键，广告条消失
      await engine.key('RIGHT', 1_000)
      await engine.delay(1_000);
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const afterRight = await engine.screen("after-right");
        // 消失检查
        expect(afterRight.pixel(110, 27)).not.toEqual([128, 48, 40]);
        expect(afterRight.pixel(10, 41)).not.toEqual(wake.pixel(10, 41));
      }, {
        timeout: 5_000,
        interval: 1_000
      })
    }
  });
});
