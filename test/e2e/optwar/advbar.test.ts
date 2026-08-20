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

  it("点击广告", async () => {
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
      console.info('移动光标到广告', Date.now())
      // 光标移动到广告
      await engine.key('UP', 1_000)
      await engine.delay(100)
    }
    {
      console.info('进入广告', Date.now())
      // 进入广告
      await engine.key('ENTER', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('ad-plugin-notice')
        // rgb(120, 124, 120)
        expect(screen.pixel(188, 105)).toEqual([120, 124, 120])
        // rgb(232, 240, 248)
        expect(screen.pixel(141, 264)).toEqual([232, 240, 248])
      }, {
        timeout: 1_000,
        interval: 1_000
      })
    }
    {
      // 下载插件
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('download-plugin-end')
        // rgb(0, 0, 0)
        expect(screen.pixel(129, 145)).toEqual([0, 0, 0])
        // rgb(232, 240, 248)
        expect(screen.pixel(66, 200)).toEqual([232, 240, 248])
        // rgb(40, 176, 216)
        expect(screen.pixel(82, 293)).toEqual([40, 176, 216])
        // rgb(248, 252, 248)
        expect(screen.pixel(30, 301)).toEqual([248, 252, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 启动插件
      await engine.key('LEFT_SOFT', 1_000)
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('open-brw-plugin')
        // rgb(248, 252, 248)
        expect(screen.pixel(56, 231)).toEqual([248, 252, 248])
        // rgb(80, 156, 224)
        expect(screen.pixel(65, 306)).toEqual([80, 156, 224])
      }, {
        timeout: 60_000,
        interval: 1_000
      })
    }
    {
      // 打开菜单
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('open-brw-menu')
        // rgb(224, 240, 248)
        expect(screen.pixel(64, 242)).toEqual([224, 240, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 选择返回
      await engine.key('UP', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('open-brw-back-select')
        // rgb(128, 192, 240)
        expect(screen.pixel(62, 284)).toEqual([128, 192, 240])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 返回
      await engine.key('ENTER', 1_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('full-power-2')
        // rgb(200, 252, 248)
        expect(screen.pixel(213, 151)).toEqual([200, 252, 248])
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      await engine.key('DOWN', 1_000)
      await engine.delay(1_000);
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
  });
});
