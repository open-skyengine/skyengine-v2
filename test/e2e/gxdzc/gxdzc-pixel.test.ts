import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

describe("gxdzc pixel flow", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("checks screen pixels after each click", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/gxdzc.mrp", { workDir: ws.dir });

    const boot = await engine.screen("boot");
    expect(boot.pixel(120, 160)).toEqual([0, 0, 0]);

    await engine.click(0, 0, 3_000);
    const wake = await engine.screen("after-wake");
    expect(wake.pixel(120, 160)).toEqual([0, 0, 0]);

    await engine.delay(5000);
    await engine.click(228, 309, 3_000);
    await engine.delay(5000);
    const menu = await engine.screen("after-menu");
    // 点击 (228,309) 后进入标题界面（"侠盗之城 / 开始游戏"），(80,80) 落在标题背景上。
    expect(menu.pixel(80, 80)).toEqual([208, 164, 72]);
  });
});
