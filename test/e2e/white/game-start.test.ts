import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage, type Rgb } from "../engine-e2e.js";

function hasColor(
  screen: PpmImage,
  expected: Rgb,
  rect: { x: number; y: number; width: number; height: number },
): boolean {
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      const pixel = screen.pixel(x, y);
      if (pixel[0] === expected[0] && pixel[1] === expected[1] && pixel[2] === expected[2]) {
        return true;
      }
    }
  }
  return false;
}

describe("white 开始游戏", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("显示软件注册提示并可返回主菜单", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });

    const mainMenu = await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "game-start-main-menu",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    await engine.click(120, 103, 1_000);

    const green: Rgb = [0, 252, 0];
    const registration = await engine.waitForScreen(
      screen => screen.uniqueColorCount() === 2
        && screen.pixel(0, 26).toString() === green.toString()
        && screen.pixel(0, 294).toString() === green.toString(),
      { name: "game-start-registration", timeoutMs: 30_000, intervalMs: 1_000 },
    );
    expect(hasColor(registration, green, { x: 7, y: 6, width: 96, height: 16 })).toBe(true);
    expect(hasColor(registration, green, { x: 7, y: 32, width: 220, height: 64 })).toBe(true);
    expect(hasColor(registration, green, { x: 4, y: 299, width: 40, height: 16 })).toBe(true);
    expect(hasColor(registration, green, { x: 196, y: 299, width: 40, height: 16 })).toBe(true);

    await engine.key("RIGHT_SOFT", 1_000);
    const returnedMenu = await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "game-start-returned-menu",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    expect(returnedMenu.diffPixelCount(mainMenu)).toBe(0);
  }, 120_000);

  it("确认注册后可以开始新游戏", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });

    await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "registration-confirm-main-menu",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    await engine.click(120, 103, 1_000);
    await engine.waitForScreen(
      screen => hasColor(screen, [0, 252, 0], { x: 4, y: 299, width: 40, height: 16 })
        && hasColor(screen, [0, 252, 0], { x: 196, y: 299, width: 40, height: 16 }),
      { name: "registration-confirm-prompt", timeoutMs: 30_000, intervalMs: 1_000 },
    );

    await engine.click(20, 306, 1_000);
    const newGameMenu = await engine.waitForPixel(144, 50, [0, 0, 248], {
      name: "registration-confirm-new-game-menu",
      timeoutMs: 30_000,
      intervalMs: 250,
    });
    expect(newGameMenu.uniqueColorCount()).toBe(3);
    expect(hasColor(newGameMenu, [0, 252, 0], { x: 4, y: 299, width: 40, height: 16 }))
      .toBe(true);
    expect(hasColor(newGameMenu, [0, 252, 0], { x: 196, y: 299, width: 40, height: 16 }))
      .toBe(true);

    await engine.click(20, 306, 1_000);
    const board = await engine.waitForPixel(120, 10, [16, 192, 240], {
      name: "registration-confirm-board",
      timeoutMs: 30_000,
      intervalMs: 250,
    });
    expect(board.uniqueColorCount()).toBeGreaterThan(100);
    expect(board.pixel(10, 50)).toEqual([240, 180, 24]);
    expect(board.pixel(10, 300)).toEqual([136, 208, 248]);
  }, 120_000);
});
