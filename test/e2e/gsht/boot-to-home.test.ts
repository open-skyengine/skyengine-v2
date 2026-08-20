import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("gsht", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("应用正常启动", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    
    // gsht
    engine = await SkyEngineE2e.start("test/fixtures/gsht_v1015.mrp", { workDir: ws.dir, memory: '2M' });

    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        const screen = await engine.screen("bgm-select");
        // rgb(248, 252, 248)
        expect(screen.pixel(145, 158)).toEqual([248, 252, 248]);
      }, {
        timeout: 30_000,
        interval: 1000
      })
    }
  });
  
});
