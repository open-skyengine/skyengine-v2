import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";
import { readFile } from "fs/promises";

describe("gwyaz", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("启动卡死检查", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();

    engine = await SkyEngineE2e.start("test/fixtures/gwyaz.mrp", { workDir: ws.dir });
    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("Engine not started");
        const boot = await engine.screen("home");
        // rgb(32, 160, 224)
        expect(boot.pixel(89, 266)).toEqual([32, 160, 224]);
      }, {
        timeout: 60_000,
        interval: 1000
      })
    }
    {
      await engine.key('RIGHT_SOFT', 1_000)
      await vi.waitFor(async () => {
        if (!engine) throw new Error("Engine not started");
        const boot = await engine.screen("not-register");
        // rgb(32, 160, 224)
        expect(boot.pixel(89, 266)).not.toEqual([32, 160, 224]);
      }, {
        timeout: 60_000,
        interval: 1000
      })
    }
  });
});
