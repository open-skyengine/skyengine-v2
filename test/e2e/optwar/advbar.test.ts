import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import { isPluginPrompt } from "./visual.js";
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

  it("广告选项可确认且运行时保持存活", async () => {
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
    fs.rmSync(ws.path('mythroad/plugins/embrw.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/plugins/dump0'), { force: true });
    engine = await SkyEngineE2e.start("test/fixtures/optwar.mrp", { workDir: ws.dir });

    const boot = await engine.waitForScreen(
      screen => screen.pixel(227, 301).toString() === "248,0,0",
      { name: "bgm-select", timeoutMs: 60_000, intervalMs: 250 },
    );
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
    {
      // 打开游戏内菜单
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('game-menu')
        // rgb(48, 188, 248)
        expect(screen.pixel(175, 103)).toEqual([48, 188, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 回车购买火力全开
      await engine.key('ENTER', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('full-power')
        // rgb(200, 252, 248)
        expect(screen.pixel(213, 151)).toEqual([200, 252, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      const fullPower = await engine.screen('full-power-before-ad')
      // 商品详情中广告位于当前项目上方。
      await engine.key('UP', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('ad-selected')
        expect(screen.pixel(0, 0)).toEqual([104, 184, 224])
        expect(screen.pixel(0, 0)).not.toEqual(fullPower.pixel(0, 0))
      }, {
        timeout: 3_000,
        interval: 1_000
      })

      // 真机崩溃发生在这里：选中广告后确认。缺少 embrw 时应进入插件下载提示，
      // 不能把失败句柄当成浏览器包并在换页后跳回已被覆盖的游戏代码。
      await engine.key('ENTER', 3_000)
      const downloadPrompt = await engine.waitForScreen(isPluginPrompt, {
        name: 'browser-plugin-download',
        timeoutMs: 3_000,
        intervalMs: 100,
      })
      expect(isPluginPrompt(downloadPrompt)).toBe(true)
      expect(fs.existsSync(ws.path('mythroad/plugins/dump0'))).toBe(false)

      await engine.key('RIGHT_SOFT', 3_000)
      await engine.waitForScreen(
        screen => screen.diffPixelCount(fullPower, { x: 0, y: 40, width: 240, height: 280 }) === 0,
        { name: 'full-power-after-download-cancel', timeoutMs: 3_000, intervalMs: 100 },
      )

      // 关闭恢复到前台的广告层后，底层游戏仍能接收焦点移动并回到商品项。
      await engine.key('RIGHT', 1_000)
      await engine.waitForScreen(
        screen => screen.pixel(0, 0).toString() === '104,184,224'
          && screen.diffPixelCount(fullPower, { x: 0, y: 40, width: 240, height: 280 }) === 0,
        { name: 'ad-selected-after-plugin-close', timeoutMs: 3_000, intervalMs: 100 },
      )
      await engine.key('DOWN', 1_000)
      await engine.waitForScreen(
        screen => screen.diffPixelCount(fullPower) === 0,
        { name: 'full-power-after-plugin-close', timeoutMs: 3_000, intervalMs: 100 },
      )

      await engine.stop()
      const stderr = fs.readFileSync(engine.stderrPath, 'utf8')
      expect(stderr).not.toMatch(/(?:MR|ARM) fault/)
    }
  });
});
