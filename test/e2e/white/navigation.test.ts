import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage, type Rgb } from "../engine-e2e.js";

function hasColor(
  screen: PpmImage,
  expected: Rgb,
  rect: { x: number; y: number; width: number; height: number },
): boolean {
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      if (screen.pixel(x, y).toString() === expected.toString()) return true;
    }
  }
  return false;
}

describe("white 主菜单导航", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("操作说明可以返回并正常退出游戏", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });
    await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "navigation-main-menu",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });

    await engine.key("DOWN", 1_000);
    await engine.key("DOWN", 1_000);
    await engine.key("ENTER", 1_000);
    const instructions = await engine.waitForScreen(
      screen => screen.uniqueColorCount() === 2
        && screen.pixel(0, 26).toString() === "0,252,0"
        && screen.pixel(0, 294).toString() === "0,252,0",
      { name: "navigation-instructions", timeoutMs: 30_000, intervalMs: 250 },
    );
    expect(hasColor(instructions, [0, 252, 0], { x: 7, y: 6, width: 96, height: 16 }))
      .toBe(true);
    expect(hasColor(instructions, [0, 252, 0], { x: 7, y: 32, width: 220, height: 64 }))
      .toBe(true);
    expect(hasColor(instructions, [0, 252, 0], { x: 4, y: 299, width: 40, height: 16 }))
      .toBe(false);
    expect(hasColor(instructions, [0, 252, 0], { x: 196, y: 299, width: 40, height: 16 }))
      .toBe(true);

    await engine.key("RIGHT_SOFT", 1_000);
    await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "navigation-returned-menu",
      timeoutMs: 30_000,
      intervalMs: 250,
    });

    await engine.key("DOWN", 1_000);
    await engine.key("DOWN", 1_000);
    await engine.key("ENTER", { timeoutMs: 5_000, waitForDraw: false });
    expect(await engine.waitForExit(30_000)).toBe(true);
  }, 120_000);
});
