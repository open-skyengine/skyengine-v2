import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("gzwdzjs 进入主菜单", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏教程开始", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path("mythroad/plugins/netpay.mrp"), { recursive: true, force: true });
    // gzwdzjs 教程开始时的场景分配超过 1MB 默认应用堆:分配失败后游戏
    // 不检查返回值,拿垃圾指针调 DrawBitmap 导致崩溃(真机大内存下正常)。
    engine = await SkyEngineE2e.start("test/fixtures/gzwdzjs.mrp", { workDir: ws.dir, memory: "2M" });

    await engine.delay(5_000);

    // 背景音乐
    const boot = await engine.screen("bgm-select");
    // 
    expect(boot.pixel(94, 59)).toEqual([0, 0, 0]);
    // rgb(0, 252, 24)
    expect(boot.pixel(132, 158)).toEqual([0, 252, 24]);

    {
      // 是否开启音乐？-> 否
      await engine.key('RIGHT_SOFT', 1_000);
      await engine.delay(3_000);
      // 进入主菜单
      const screen = await engine.screen("menu");
      // rgb(232, 176, 152)
      expect(screen.pixel(169, 117)).toEqual([232, 176, 152]);
      // rgb(152, 228, 0)
      expect(screen.pixel(38, 22)).toEqual([152, 228, 0]);
    }
    {
      console.info('开始游戏')
      // 回车
      for (let i = 0; i < 3; i++) {
        await engine.key('ENTER', 1_000);
        await engine.delay(1_000);
      }
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("need-power");
        // rgb(208, 244, 200)
        expect(screen.pixel(94, 145)).toEqual([208, 244, 200]);
      }, { timeout: 30_000, interval: 1_000 });
    }
    {
      console.info('开始游戏')
      // 回车
      for (let i = 0; i < 3; i++) {
        await engine.key('ENTER', 1_000);
        await engine.delay(1_000);
      }
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("need-power");
        // rgb(200, 204, 248)
        expect(screen.pixel(94, 145)).toEqual([200, 204, 248]);
      }, { timeout: 30_000, interval: 1_000 });
    }
    {
      // 反抗是没用的
      await engine.key('ENTER', 1_000);
      await engine.delay(1_000);
      console.info('等待演示动画')
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("need-power");
        // rgb(208, 244, 200)
        expect(screen.pixel(94, 145)).toEqual([208, 244, 200]);
      }, { timeout: 180_000, interval: 1_000 });
    }
    {
      console.info('继续游戏')
      // 5下回车
      for (let i = 0; i < 5; i++) {
        await engine.key('ENTER', 1_000);
        await engine.delay(1_000);
      }
      const screen = await engine.screen("tutorial");
      // rgb(208, 244, 200)
      expect(screen.pixel(97, 166)).toEqual([208, 244, 200]);
      // rgb(0, 0, 0)
      expect(screen.pixel(121, 57)).toEqual([0, 0, 0]);
    }
    {
      console.info('左软键确定开始教程')
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(5_000);
      const screen = await engine.screen("start-confirm");
      expect(screen.pixel(75, 75)).not.toEqual([0, 0, 0]);
    }
    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("introduce");
        // rgb(208, 244, 200)
        expect(screen.pixel(94, 145)).toEqual([208, 244, 200]);
      }, { timeout: 90_000, interval: 1_000 });
      for (let i = 0; i < 2; i++) {
        await engine.key('ENTER', 1_000);
        await engine.delay(1_000);
      }
      const screen = await engine.screen("game-start");
      expect(screen.pixel(75, 75)).not.toEqual([0, 0, 0]);
      expect(screen.pixel(43, 149)).not.toEqual([208, 244, 200]);
      // rgb(24, 12, 0)
      expect(screen.pixel(42, 245)).toEqual([24, 12, 0]);
    }
  });
  it("花屏检查", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // gzwdzjs 教程开始时的场景分配超过 1MB 默认应用堆:分配失败后游戏
    // 不检查返回值,拿垃圾指针调 DrawBitmap 导致崩溃(真机大内存下正常)。
    engine = await SkyEngineE2e.start("test/fixtures/gzwdzjs.mrp", { workDir: ws.dir, memory: "2M" });

    await engine.delay(5_000);

    // 背景音乐
    const boot = await engine.screen("bgm-select");
    // 
    expect(boot.pixel(94, 59)).toEqual([0, 0, 0]);
    // rgb(0, 252, 24)
    expect(boot.pixel(132, 158)).toEqual([0, 252, 24]);

    {
      // 是否开启音乐？-> 否
      await engine.key('RIGHT_SOFT', 1_000);
      await engine.delay(3_000);
      // 进入主菜单
      const screen = await engine.screen("menu");
      // rgb(232, 176, 152)
      expect(screen.pixel(169, 117)).toEqual([232, 176, 152]);
      // rgb(152, 228, 0)
      expect(screen.pixel(38, 22)).toEqual([152, 228, 0]);
    }
    {
      // 2下回车
      for (let i = 0; i < 4; i++) {
        await engine.key('ENTER', 1_000);
        await engine.delay(1_000);
      }
      const screen = await engine.screen("render-check");
      // rgb(96, 60, 0)
      expect(screen.pixel(128, 276)).toEqual([96, 60, 0]);
    }
  });
});
