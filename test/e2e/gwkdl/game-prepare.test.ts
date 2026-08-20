import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("gwkdl 进入准备界面", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏准备 - 界面正常", async () => {
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
    fs.rmSync(ws.path('mythroad/cache/exdet.dat'), { force: true })
    engine = await SkyEngineE2e.start("test/fixtures/gwkdl_v1003.mrp", { workDir: ws.dir });

    await engine.delay(1000);
    {
      const boot = await engine.screen("mem-check");
      // rgb(248, 252, 248)
      expect(boot.pixel(60, 82)).toEqual([248, 252, 248]);
    }
    // 是否检测内存？-> 是
    await engine.key('LEFT_SOFT', 1_000);
    await engine.delay(15_000);
    const boot = await engine.screen("bgm-select");
    expect(boot.pixel(150, 308)).toEqual([0, 0, 0]);
    // rgb(248, 240, 0)
    expect(boot.pixel(116, 84)).toEqual([248, 240, 0]);

    // 是否开启音乐？-> 否
    await engine.click(227, 301, 1_000);
    await engine.delay(3_000);

    // 进入“请按任意键界面”
    const wake = await engine.screen("menu");
    // 图片资源缺失时该画面只剩背景色和少量文字色；正常加载后会出现位图的多色像素。
    expect(wake.uniqueColorCount()).toBeGreaterThan(16);
  });
});
