import { afterEach, describe, expect, it } from "vitest";
import {
  SkyEngineE2e,
  SkyEngineWorkspace,
  type PpmImage,
  type Rgb,
} from "../engine-e2e.js";

const MENU_BACKGROUND = [24, 160, 200] as const satisfies Rgb;
const SELECTED_BACKGROUND = [216, 228, 240] as const satisfies Rgb;

function differingPixels(
  screen: PpmImage,
  rect: { x: number; y: number; width: number; height: number },
  background: Rgb,
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      const pixel = screen.pixel(x, y);
      if (pixel[0] !== background[0] || pixel[1] !== background[1] || pixel[2] !== background[2]) {
        count++;
      }
    }
  }
  return count;
}

function expectSelectedMenuRow(screen: PpmImage, y: number): void {
  // The selection background starts after the icon and extends beyond the text.
  expect(screen.pixel(150, y + 8)).toEqual(SELECTED_BACKGROUND);
  expect(differingPixels(screen, { x: 5, y, width: 16, height: 16 }, MENU_BACKGROUND)).toBeGreaterThan(8);
  expect(differingPixels(screen, { x: 26, y, width: 96, height: 16 }, SELECTED_BACKGROUND)).toBeGreaterThan(16);
}

describe("dsm_gm", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("向下移动后完整绘制焦点行", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/dsm_gm.mrp", {
      workDir: ws.dir,
      dnsMap:
        "rop.skymobiapp.com->159.75.119.124;" +
        "spd.skymobiapp.com->159.75.119.124;" +
        "proxy.51mrp.com->127.0.0.1;" +
        "proxy2.51mrp.com->127.0.0.1",
    });

    const initial = await engine.waitForScreen(
      (screen) =>
        screen.pixel(150, 53).toString() === SELECTED_BACKGROUND.toString() &&
        differingPixels(screen, { x: 5, y: 45, width: 16, height: 16 }, MENU_BACKGROUND) > 8,
      { name: "initial-menu", timeoutMs: 10_000, intervalMs: 250 },
    );
    expectSelectedMenuRow(initial, 45);

    await engine.key("DOWN", { timeoutMs: 1_000, holdMs: 80 });
    await engine.delay(250);
    const movedOnce = await engine.screen("moved-once-menu");
    expectSelectedMenuRow(movedOnce, 65);

    await engine.key("DOWN", { timeoutMs: 1_000, holdMs: 80 });
    await engine.delay(250);
    const movedTwice = await engine.screen("moved-twice-menu");
    expectSelectedMenuRow(movedTwice, 85);
  });
});
