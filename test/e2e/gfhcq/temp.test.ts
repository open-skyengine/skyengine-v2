import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";
import { readFile } from "fs/promises";

describe("gfhcq", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("卡顿", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/brw'), { recursive: true, force: true })

    // 480x800 RGB565 屏幕本身占约 750 KiB；显式使用复现环境的 8 MiB
    // 档位，避免默认 1 MiB 把资源内存不足混入游戏阶段的性能测量。
    engine = await SkyEngineE2e.start("test/fixtures/gfhcq.mrp", {
      workDir: ws.dir,
      screen: '480x800',
      memory: '8M'
    });
    {
      await engine.delay(2_000);
      await vi.waitFor(async () => {
        if (!engine) throw new Error('skyengine not defined')
        const screen = await engine.screen('bgm-select')
        // rgb(248, 252, 248)
        expect(screen.pixel(256, 406)).toEqual([248, 252, 248])
      }, { timeout: 30_000, interval: 500 })
    }
    {
      // 不开启BGM
      await engine.key('RIGHT_SOFT', 1_000)
      await vi.waitFor(async () => {
        if (!engine) throw new Error('skyengine not defined')
        const screen = await engine.screen('menu')
        // rgb(8, 8, 144)
        expect(screen.pixel(327, 583)).toEqual([8, 8, 144])
      }, { timeout: 30_000, interval: 500 })
    }
    {
      await engine.delay(3_000)
      await engine.key('ENTER', 1_000)
      // 用低频 draw-count 和首尾 PPM 衡量游戏实际推进速度，避免开启海量
      // ARM trace 后由日志 I/O 反过来污染卡顿基线。
      const firstFrame = await engine.screen('game-start')
      const firstDrawCount = await engine.drawCount()
      const samples = [firstDrawCount]
      for (let elapsed = 10_000; elapsed <= 60_000; elapsed += 10_000) {
        await engine.delay(10_000)
        samples.push(await engine.drawCount())
      }
      const lastFrame = await engine.screen('game-60s')
      console.info(
        `[gfhcq-perf] draw-counts=${samples.join(',')} ` +
        `changed-pixels=${firstFrame.diffPixelCount(lastFrame)}`
      )
    }
  });
});
