import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

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
    // 删除后，启动游戏会自动下载。
    fs.rmSync(ws.path('mythroad/plugins/advbar.mrp'), { force: true });
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
  it("游戏退出时下载插件", async () => {
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
      console.info("等待下载完成，下载完成后会自动启动营销商店");
      await engine.delay(5_000);
      console.info("下载完成");
      const screen = await engine.screen("download-result");
      // rgb(248, 0, 0)
      expect(screen.pixel(134, 146)).not.toEqual([248, 0, 0]);
      // rgb(0, 0, 0)
      expect(screen.pixel(107, 151)).toEqual([0, 0, 0]);
      // rgb(248, 252, 248)
      expect(screen.pixel(32, 301)).toEqual([248, 252, 248]);
    }
    {
      // 启动营销商店。这个按键会先进入插件文件/网络启动路径，不保证在
      // 按键命令的 3s 窗口内重绘；回归点是后续 ARM 侧启动不应再异常退出。
      console.info("启动营销商店");
      await engine.key('LEFT_SOFT', 3_000)
      await engine.delay(20_000);
      // 会请求dl_confirm
      const screen = await engine.screen("promote-start");
      expect(screen.pixel(104, 296)).not.toEqual([0, 0, 0]);
    }
  });
});
