import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("dota community", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("访问浏览器插件", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载浏览器插件界面。
    fs.rmSync(ws.path('mythroad/plugins/embrw.mrp'), { force: true });
    engine = await SkyEngineE2e.start("test/fixtures/dota.mrp", {
      workDir: ws.dir,
      dnsMap: [
        'rop.skymobiapp.com->159.75.119.124;spd.skymobiapp.com->159.75.119.124;proxy.51mrp.com->127.0.0.1;proxy2.51mrp.com->127.0.0.1'
      ].join(';')
    });

    await engine.delay(6000);
    const boot = await engine.screen("bgm-select");
    // rgb(216, 24, 96)
    expect(boot.pixel(229, 306)).toEqual([216, 24, 96]);

    // 是否开启音乐？-> 否
    await engine.click(228, 308, 1_000);
    await engine.delay(2_000);

    // 任意键进入主菜单
    await engine.click(50, 50, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(144, 20, 40)
    expect(wake.pixel(202, 203)).toEqual([144, 20, 40]);
    // rgb(40, 8, 16)
    expect(wake.pixel(205, 297)).toEqual([40, 8, 16]);
    {
      // 切换菜单到游戏社区
      await engine.key('UP', 3_000);
      await engine.delay(1_000);
      await engine.key('UP', 3_000);
      await engine.delay(1_000);
      const screen = await engine.screen("menu");
      // rgb(216, 32, 80)
      expect(screen.pixel(57, 222)).toEqual([216, 32, 80]);
    }
    {
      // 点击确定，进入插件下载界面
      await engine.key('LEFT_SOFT', 3_000);
      await engine.delay(1_000);
      const screen = await engine.screen("download-plugin");
      // rgb(0, 4, 0)
      expect(screen.pixel(80, 80)).toEqual([0, 4, 0]);
    }
    {
      // 点击确定，下载浏览器插件
      await engine.key('LEFT_SOFT', 3_000);
      await engine.delay(1_000);
      await vi.waitFor(async () => {
        const screen = await engine!.screen("download-end");
        // rgb(0, 252, 0)
        expect(screen.pixel(137, 149)).toEqual([0, 252, 0]);
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      await engine.delay(500)
      await engine.key('LEFT_SOFT', 3_000);
      await engine.delay(1_000);
      await engine.delay(500)
      await engine.key('ENTER', 3_000);
      await engine.delay(500)
      await vi.waitFor(async () => {
        const screen = await engine!.screen("brw-opened");
        // rgb(240, 244, 240)
        expect(screen.pixel(71, 69)).toEqual([240, 244, 240]);
      }, {
        timeout: 30_000,
        interval: 1_000
      })
    }
    {
      // 失效源站可能直到代理的 15 秒请求超时后才返回错误页。
      await engine.delay(20_000)
      const screen = await engine!.screen("url-opened");
    }
  });

});
