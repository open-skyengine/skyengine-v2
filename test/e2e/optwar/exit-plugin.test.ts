import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

function countColor(
  image: Awaited<ReturnType<SkyEngineE2e["screen"]>>,
  color: readonly [number, number, number],
  rect: { x: number; y: number; width: number; height: number },
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      if (image.pixel(x, y).toString() === color.toString()) count += 1;
    }
  }
  return count;
}

describe("optwar 进入主菜单", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("advbar", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // advbar 自身拥有更新逻辑；主应用不会在插件完全缺失时引导下载。
    fs.cpSync('test/fixtures/plugins/advbar.mrp', ws.path('mythroad/plugins/advbar.mrp'));
    engine = await SkyEngineE2e.start("test/fixtures/optwar.mrp", { workDir: ws.dir });

    await engine.delay(10000);
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

  });
  it("游戏退出时处理插件下载失败", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    if (!fs.existsSync(ws.path('mythroad/plugins/netpay.mrp'))) {
      fs.cpSync('test/fixtures/plugins/netpay.mrp', ws.path('mythroad/plugins/netpay.mrp'));
    }
    fs.rmSync(ws.path('mythroad/plugins/promote.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/plugins/brwcore.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/promote'), { force: true, recursive: true });
    // 本用例验证前台 advbar 插件与游戏主画面的屏幕合成，运行前显式准备插件资源。
    if (!fs.existsSync(ws.path('mythroad/plugins/advbar.mrp'))) {
      fs.cpSync('test/fixtures/plugins/advbar.mrp', ws.path('mythroad/plugins/advbar.mrp'));
    }
    engine = await SkyEngineE2e.start("test/fixtures/optwar.mrp", {
      workDir: ws.dir,
      // 把该游戏使用过的下载入口都固定到本机拒绝端口，避免测试依赖外网。
      dnsMap: "10.0.0.172->127.0.0.1:1;spd.skymobiapp.com->127.0.0.1:1;rop.skymobiapp.com->127.0.0.1:1",
    });

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

      // 下一次方向键才进入游戏菜单状态机，避免同一个按键被原始事件和 Lua 转发重复处理。
      await engine.key('RIGHT', 1_000)
      await engine.delay(1_000);
      const afterSecondRight = await engine.screen("after-second-right");
      expect(afterSecondRight.pixel(98, 264)).not.toEqual(afterRight.pixel(98, 264));
    }
    {
      // 切换到退出选项
      for (let i = 0; i < 5; i++) {
        await engine.key('RIGHT', 1_000)
        await engine.delay(1_000);
      }
    }
    {
      // 下载目标：plugins/promote.mrp
      // 确认退出
      await engine.key('LEFT_SOFT', 1_000)
      await engine.delay(1_000);
      const screen = await engine.screen("download-notice");
      // rgb(232, 240, 248)
      expect(screen.pixel(117, 258)).toEqual([232, 240, 248]);
      // rgb(40, 176, 216)
      expect(screen.pixel(104, 296)).toEqual([40, 176, 216]);
    }
    {
      // 点击确认后，开始下载
      await engine.key('LEFT_SOFT', 1_000)
      console.info("等待本地下载失败结果");
      await engine.delay(5_000);
      const screen = await engine.screen("download-result");
      // rgb(248, 0, 0)
      expect(screen.pixel(134, 146)).toEqual([248, 0, 0]);
      expect(countColor(screen, [248, 0, 0], { x: 80, y: 144, width: 80, height: 18 })).toBeGreaterThan(250);
      // rgb(40, 176, 216)
      expect(screen.pixel(32, 301)).toEqual([40, 176, 216]);
      expect(fs.existsSync(ws.path('mythroad/plugins/promote.mrp'))).toBe(false);
    }
  });
});
