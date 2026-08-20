import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from 'fs'

describe("talkcat 进入游戏", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏启动正常", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/talkcat'), { force: true, recursive: true })

    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", { workDir: ws.dir });

    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("main");
      // rgb(232, 236, 232)
      expect(boot.pixel(27, 273)).toEqual([232, 236, 232]);
      // rgb(0, 12, 16)
      expect(boot.pixel(216, 27)).toEqual([0, 12, 16]);
      // rgb(64, 64, 64)
      expect(boot.pixel(221, 279)).toEqual([64, 64, 64]);
    }, { timeout: 90_000, interval: 1_000 });
  });
  it("下载喝水资源包并安装", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/talkcat'), { force: true, recursive: true })

    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", { workDir: ws.dir });

    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("main");
      // rgb(232, 236, 232)
      expect(boot.pixel(27, 273)).toEqual([232, 236, 232]);
      // rgb(0, 12, 16)
      expect(boot.pixel(216, 27)).toEqual([0, 12, 16]);
      // rgb(64, 64, 64)
      expect(boot.pixel(221, 279)).toEqual([64, 64, 64]);
    }, { timeout: 90_000, interval: 1_000 });

    {
      // 点击水杯图标，触发下载提示
      await engine.click(22, 280, 1_000)
      await engine.delay(1_000)
      // 检查像素
      const screen = await engine.screen("download-confirm");
      // rgb(32, 64, 120)
      expect(screen.pixel(78, 280)).toEqual([32, 64, 120]);
    }
    {
      // 点击确定开始下载
      await engine.click(78, 280, 1_000)
      await engine.delay(5_000)
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("downloading");
        // rgb(32, 212, 0)
        expect(screen.pixel(79, 257)).toEqual([32, 212, 0]);
      }, {
        timeout: 20_000,
        interval: 1_000
      })
    }
    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        // 检查是否显示安装提示
        // rgb(32, 64, 120)
        const screen = await engine.screen("install-confirm");
        expect(screen.pixel(77, 279)).toEqual([32, 64, 120]);
      }, {
        timeout: 30_000,
        interval: 1_000
      })
    }
    {
      // 点击确定开始安装
      await engine.click(77, 279, 1_000)
      
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        // rgb(32, 212, 0)
        const screen = await engine.screen("installing");
        expect(screen.pixel(54, 257)).toEqual([32, 212, 0]);
      }, {
        timeout: 30_000,
        interval: 1_000
      })

      await engine.delay(80_000)
      const stdout = fs.readFileSync(engine.stdoutPath, 'utf-8')
      expect(stdout).not.contain('Invalid memory read')
    }
  });
  it("循环取消", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/talkcat'), { force: true, recursive: true })

    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", { workDir: ws.dir });

    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("main");
      // rgb(232, 236, 232)
      expect(boot.pixel(27, 273)).toEqual([232, 236, 232]);
      // rgb(0, 12, 16)
      expect(boot.pixel(216, 27)).toEqual([0, 12, 16]);
      // rgb(64, 64, 64)
      expect(boot.pixel(221, 279)).toEqual([64, 64, 64]);
    }, { timeout: 90_000, interval: 1_000 });
    for (let i = 0; i < 20; i++) {
      {
        // 点击水杯图标，触发下载提示
        await engine.click(139, 266, 1_000)
        await engine.delay(1_000)
        // 检查像素
        const screen = await engine.screen("download-confirm");
        // rgb(32, 64, 120)
        expect(screen.pixel(78, 280)).toEqual([32, 64, 120]);
      }
      {
        // 点击确定开始下载
        await engine.click(139, 266, 1_000)
        await engine.delay(1_000)
        // rgb(32, 212, 0)
        const screen = await engine.screen("download-cancel");
        // rgb(32, 64, 120)
        expect(screen.pixel(78, 280)).not.toEqual([32, 64, 120]);
        await engine.delay(1_000)
      }
    }
  });
});
