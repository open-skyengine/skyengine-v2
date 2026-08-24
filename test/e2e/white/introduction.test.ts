import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

describe("white 游戏介绍", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("长文本可以向下滚动并回到首屏", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });

    await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "introduction-main-menu",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    await engine.key("DOWN", 1_000);
    await engine.key("ENTER", 1_000);

    const firstPage = await engine.waitForScreen(
      screen => screen.uniqueColorCount() === 2
        && screen.pixel(0, 26).toString() === "0,252,0"
        && screen.pixel(0, 294).toString() === "0,252,0",
      { name: "introduction-first-page", timeoutMs: 30_000, intervalMs: 1_000 },
    );
    expect(firstPage.pixel(15, 296)).toEqual([0, 0, 0]);
    expect(firstPage.pixel(234, 32)).toEqual([0, 252, 0]);
    expect(firstPage.pixel(234, 293)).toEqual([0, 0, 0]);

    let previousPage = firstPage;
    for (let page = 1; page <= 5; page++) {
      await engine.key("DOWN", 1_000);
      const currentPage = await engine.screen(`introduction-page-${page}`);
      expect(currentPage.diffPixelCount(previousPage, { x: 0, y: 27, width: 240, height: 267 }))
        .toBeGreaterThan(0);
      expect(currentPage.diffPixelCount(firstPage, { x: 0, y: 0, width: 240, height: 27 }))
        .toBe(0);
      expect(currentPage.diffPixelCount(firstPage, { x: 0, y: 294, width: 240, height: 26 }))
        .toBe(0);
      previousPage = currentPage;
    }
    expect(previousPage.pixel(234, 32)).toEqual([0, 0, 0]);
    expect(previousPage.pixel(234, 293)).toEqual([0, 252, 0]);

    const lastPageDraw = await engine.drawCount();
    await engine.key("DOWN", { waitForDraw: false });
    await engine.delay(100);
    expect(await engine.drawCount()).toBe(lastPageDraw);

    for (let page = 4; page >= 0; page--) {
      await engine.key("UP", 1_000);
    }
    const restoredFirstPage = await engine.screen("introduction-first-page-restored");
    expect(restoredFirstPage.diffPixelCount(firstPage)).toBe(0);

    await engine.key("RIGHT_SOFT", 1_000);
    await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "introduction-returned-menu",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
  }, 120_000);
});
