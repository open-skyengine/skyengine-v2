import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("wbrw 进入主界面", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("浏览器正常启动", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/brw'), { recursive: true, force: true })
    
    engine = await SkyEngineE2e.start("test/fixtures/wbrw.mrp", { workDir: ws.dir });

    await engine.delay(5000);
    const boot = await engine.screen("home");
    // rgb(248, 252, 248)
    expect(boot.pixel(76, 252)).toEqual([248, 252, 248]);
    // rgb(64, 144, 208)
    expect(boot.pixel(57, 306)).toEqual([64, 144, 208]);

  });
});
