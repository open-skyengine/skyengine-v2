import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

describe("gjxwsmn", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("启动", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/gjxwsmn.mrp", { workDir: ws.dir });

    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("bgm-select");
        // rgb(232, 48, 0)
        expect(screen.pixel(147, 87)).toEqual([232, 48, 0]);
        // rgb(176, 192, 208)
        expect(screen.pixel(116, 122)).toEqual([176, 192, 208]);
      }, { timeout: 10_000, interval: 1_000 });
    }
    {
      await engine.key('RIGHT_SOFT', 1_000);

      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("menu");
        // rgb(24, 8, 16)
        expect(screen.pixel(206, 44)).toEqual([24, 8, 16]);
        // rgb(248, 192, 192)
        expect(screen.pixel(74, 219)).toEqual([248, 192, 192]);
      }, { timeout: 10_000, interval: 1_000 });
    }
    {
      await engine.key('RIGHT', 1_000);
      await engine.delay(1_000);
      await engine.key('ENTER', 1_000);

      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("first-scene");
        // rgb(248, 220, 144)
        expect(screen.pixel(173, 156)).toEqual([248, 220, 144]);
        // rgb(216, 228, 232)
        expect(screen.pixel(116, 55)).toEqual([216, 228, 232]);
      }, { timeout: 10_000, interval: 1_000 });
    }
  });
});
