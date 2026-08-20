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

  it("突然加速1", async () => {
    let normalDrawRate: number;
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
      // 用支付前的同一游戏场景建立绘制节奏基线。
      const normalStart = await engine.drawCount()
      const normalStartedAt = performance.now()
      await engine.delay(2_000)
      const normalEnd = await engine.drawCount()
      const normalElapsedSeconds = (performance.now() - normalStartedAt) / 1_000
      normalDrawRate = (normalEnd - normalStart) / normalElapsedSeconds
      expect(normalDrawRate).toBeGreaterThan(0)
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
      await vi.waitFor(async () => {
        const screen = await engine!.screen('select-second-method-1')
        // rgb(48, 188, 248)
        expect(screen.pixel(222, 287)).toEqual([48, 188, 248])
        // rgb(168, 20, 32)
        expect(screen.pixel(230, 20)).toEqual([168, 20, 32])
      }, {
        timeout: 3_000,
        interval: 1_000
      })
    }
    {
      // 进入广告/支付插件界面。
      await engine.key('ENTER', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('pay-scene-1')
        // rgb(232, 240, 248)
        expect(screen.pixel(218, 256)).toEqual([232, 240, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 取消支付
      const cancelStartedAt = performance.now()
      await engine.key('RIGHT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('select-second-method-2')
        // rgb(48, 188, 248)
        expect(screen.pixel(222, 287)).toEqual([48, 188, 248])
        // rgb(168, 20, 32)
        expect(screen.pixel(230, 20)).toEqual([168, 20, 32])
      }, {
        timeout: 1_000,
        interval: 1_000
      })
      // 计时必须覆盖 KEY 自身的 guest callback 边界；否则 callback 内部的
      // 主线程阻塞会发生在 waitFor 启动前，像素正确也会漏掉真实卡顿。
      expect(performance.now() - cancelStartedAt).toBeLessThan(1_000)
    }
    {
      // 返回游戏菜单
      await engine.key('RIGHT_SOFT', 1_000)
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
      // 返回游戏界面
      await engine.key('RIGHT_SOFT', 1_000)
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
      // 跳过返回边界的一次性恢复工作，再用长窗口排除持续倍速或冻结。
      await engine.delay(1_000)
      const resumedFirstFrame = await engine.screen('resume-rate-start')
      const resumedStart = await engine.drawCount()
      const resumedStartedAt = performance.now()
      await engine.delay(5_000)
      const resumedTailFirstFrame = await engine.screen('resume-rate-tail-start')
      const resumedTailStart = await engine.drawCount()
      const resumedTailStartedAt = performance.now()
      await engine.delay(2_000)
      const resumedEnd = await engine.drawCount()
      const resumedElapsedSeconds = (performance.now() - resumedStartedAt) / 1_000
      const resumedDrawRate = (resumedEnd - resumedStart) / resumedElapsedSeconds
      const resumedTailElapsedSeconds = (performance.now() - resumedTailStartedAt) / 1_000
      const resumedTailDrawRate = (resumedEnd - resumedTailStart) / resumedTailElapsedSeconds
      const resumedLastFrame = await engine.screen('resume-rate-end')

      console.info(
        `[optwar-resume-rate] baseline=${normalDrawRate.toFixed(3)} ` +
        `resumed=${resumedDrawRate.toFixed(3)} tail=${resumedTailDrawRate.toFixed(3)}`
      )
      expect(resumedDrawRate).toBeGreaterThanOrEqual(normalDrawRate * 0.5)
      expect(resumedDrawRate).toBeLessThanOrEqual(normalDrawRate * 1.5)
      expect(resumedLastFrame.diffPixelCount(resumedFirstFrame)).toBeGreaterThan(0)
      // 末段必须仍以正常节奏推进，避免前半段绘制后冻结却通过整窗平均值。
      expect(resumedTailDrawRate).toBeGreaterThanOrEqual(normalDrawRate * 0.5)
      expect(resumedTailDrawRate).toBeLessThanOrEqual(normalDrawRate * 1.5)
      expect(resumedLastFrame.diffPixelCount(resumedTailFirstFrame)).toBeGreaterThan(0)
      expect(resumedLastFrame.pixel(22, 314)).toEqual([200, 252, 248])
    }
  });
  it("突然加速2", async () => {
    let normalDrawRate: number;
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
      // 用支付前的同一游戏场景建立绘制节奏基线。
      const normalStart = await engine.drawCount()
      const normalStartedAt = performance.now()
      await engine.delay(2_000)
      const normalEnd = await engine.drawCount()
      const normalElapsedSeconds = (performance.now() - normalStartedAt) / 1_000
      normalDrawRate = (normalEnd - normalStart) / normalElapsedSeconds
      expect(normalDrawRate).toBeGreaterThan(0)
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
      await vi.waitFor(async () => {
        const screen = await engine!.screen('select-second-method-1')
        // rgb(48, 188, 248)
        expect(screen.pixel(222, 287)).toEqual([48, 188, 248])
        // rgb(168, 20, 32)
        expect(screen.pixel(230, 20)).toEqual([168, 20, 32])
      }, {
        timeout: 3_000,
        interval: 1_000
      })
    }
    {
      // 进入令牌插件下载界面
      await engine.key('ENTER', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('pay-scene-1')
        // rgb(232, 240, 248)
        expect(screen.pixel(218, 256)).toEqual([232, 240, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 确认下载
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('download-end')
        // rgb(248, 252, 248)
        expect(screen.pixel(30, 301)).toEqual([248, 252, 248])
      }, {
        timeout: 40_000,
        interval: 1_000
      })
    }
    {
      // 确定下载结果
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('select-second-method-2')
        // rgb(48, 188, 248)
        expect(screen.pixel(222, 287)).toEqual([48, 188, 248])
        // rgb(168, 20, 32)
        expect(screen.pixel(230, 20)).toEqual([168, 20, 32])
      }, {
        timeout: 3_000,
        interval: 1_000
      })
    }
    {
      // 确定付费
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('pay-success')
        // rgb(0, 0, 0)
        expect(screen.pixel(170, 132)).toEqual([0, 0, 0])
        // rgb(200, 252, 248)
        expect(screen.pixel(52, 134)).toEqual([200, 252, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 确定付费结果
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('game-menu-2')
        // rgb(48, 188, 248)
        expect(screen.pixel(175, 103)).toEqual([48, 188, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 确定付费结果
      await engine.key('RIGHT_SOFT', 1_000)
    }
    {
      // 跳过返回边界的一次性恢复工作，再用长窗口排除持续倍速或冻结。
      await engine.delay(1_000)
      const resumedFirstFrame = await engine.screen('resume-rate-start')
      const resumedStart = await engine.drawCount()
      const resumedStartedAt = performance.now()
      await engine.delay(5_000)
      const resumedTailFirstFrame = await engine.screen('resume-rate-tail-start')
      const resumedTailStart = await engine.drawCount()
      const resumedTailStartedAt = performance.now()
      await engine.delay(2_000)
      const resumedEnd = await engine.drawCount()
      const resumedElapsedSeconds = (performance.now() - resumedStartedAt) / 1_000
      const resumedDrawRate = (resumedEnd - resumedStart) / resumedElapsedSeconds
      const resumedTailElapsedSeconds = (performance.now() - resumedTailStartedAt) / 1_000
      const resumedTailDrawRate = (resumedEnd - resumedTailStart) / resumedTailElapsedSeconds
      const resumedLastFrame = await engine.screen('resume-rate-end')
      const resumedRateRatio = resumedDrawRate / normalDrawRate
      const resumedTailRateRatio = resumedTailDrawRate / normalDrawRate

      console.info(
        `[optwar-resume-rate] baseline=${normalDrawRate.toFixed(3)} ` +
        `resumed=${resumedDrawRate.toFixed(3)} tail=${resumedTailDrawRate.toFixed(3)} ` +
        `ratios=${resumedRateRatio.toFixed(3)}/${resumedTailRateRatio.toFixed(3)}`
      )

      // 完整窗口应贴近支付前节奏；25% 仅容纳墙钟和窗口边界抖动。
      expect(resumedRateRatio).toBeGreaterThanOrEqual(0.8)
      expect(resumedRateRatio).toBeLessThanOrEqual(1.2)
      expect(resumedLastFrame.diffPixelCount(resumedFirstFrame)).toBeGreaterThan(0)
      // 两秒末段存在一帧约 5% 的量化误差，10% 边界同时排除冻结和持续漂移。
      expect(resumedTailRateRatio).toBeGreaterThanOrEqual(0.8)
      expect(resumedTailRateRatio).toBeLessThanOrEqual(1.2)
      expect(resumedLastFrame.diffPixelCount(resumedTailFirstFrame)).toBeGreaterThan(0)
      expect(resumedLastFrame.pixel(22, 314)).toEqual([200, 252, 248])
    }
  });
});
