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

  it("缺少活动插件时可提示并返回", async () => {
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
    fs.rmSync(ws.path('mythroad/plugins/freecurr.mrp'), { force: true });
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
      // 打开支付方式
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('pay-method-1')
        // rgb(48, 188, 248)
        expect(screen.pixel(230, 269)).toEqual([48, 188, 248])
        // rgb(168, 20, 32)
        expect(screen.pixel(230, 20)).toEqual([168, 20, 32])
      }, {
        timeout: 3_000,
        interval: 1_000
      })
    }
    {
      // 选择令牌支付
      await engine.key('DOWN', 1_000)
      const selectedPayment = await vi.waitFor(async () => {
        const screen = await engine!.screen('select-second-method-1')
        // rgb(48, 188, 248)
        expect(screen.pixel(222, 287)).toEqual([48, 188, 248])
        // rgb(168, 20, 32)
        expect(screen.pixel(230, 20)).toEqual([168, 20, 32])
        return screen
      }, {
        timeout: 3_000,
        interval: 1_000
      })

      // 提示可能在 KEY 响应的绘图基线之后才由异步回调完成，因此由完整画面
      // 识别等待目标状态，期间不能再发送一次确认键。
      await engine.key('ENTER', { waitForDraw: false })
      const activity = await engine.waitForScreen(isPluginPrompt, {
        name: 'free-currency-activity',
        timeoutMs: 3_000,
        intervalMs: 100,
      })
      expect(isPluginPrompt(activity)).toBe(true)

      await engine.key('RIGHT_SOFT', 3_000)
      await engine.waitForScreen(
        screen => screen.diffPixelCount(selectedPayment) === 0,
        { name: 'payment-after-activity-cancel', timeoutMs: 3_000, intervalMs: 100 },
      )
    }
  });
  it("广告选项可选择并返回", async () => {
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
      // 商品详情中广告位于当前项目上方；该外部浏览器后端不属于 headless
      // 测试环境，因此这里只验证选项状态机，不伪造一次外部动作成功。
      await engine.key('UP', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('ad-selected')
        expect(screen.pixel(0, 0)).toEqual([104, 184, 224])
        expect(screen.pixel(0, 0)).not.toEqual(fullPower.pixel(0, 0))
      }, {
        timeout: 3_000,
        interval: 1_000
      })

      await engine.key('DOWN', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('full-power-returned')
        expect(screen.pixel(0, 0)).toEqual(fullPower.pixel(0, 0))
        expect(screen.pixel(213, 151)).toEqual([200, 252, 248])
      }, {
        timeout: 3_000,
        interval: 1_000
      })
    }
  });
});
