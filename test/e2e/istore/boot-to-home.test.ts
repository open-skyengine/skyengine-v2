import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("istore 进入主界面", () => {
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
    
    // istore
    engine = await SkyEngineE2e.start("test/fixtures/sky_istore.mrp", { workDir: ws.dir, memory: '2M' });

    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 无网络提示
        const screen = await engine.screen("home");
        // rgb(112, 152, 208)
        expect(screen.pixel(123, 78)).toEqual([112, 152, 208]);
        // rgb(216, 220, 216)
        expect(screen.pixel(143, 162)).toEqual([216, 220, 216]);
        // rgb(168, 208, 80)
        expect(screen.pixel(121, 205)).toEqual([168, 208, 80]);
      }, {
        timeout: 30_000,
        interval: 1000
      })
    }
  });
  
});
