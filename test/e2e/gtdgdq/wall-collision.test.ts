import fs from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

function isGameScreen(screen: Awaited<ReturnType<SkyEngineE2e["screen"]>>): boolean {
  for (let y = 260; y < 300; y += 1) {
    for (let x = 0; x < screen.width; x += 1) {
      const [red, green, blue] = screen.pixel(x, y);
      if (red === 24 && green === 120 && blue === 248) return true;
    }
  }
  return false;
}

function ballCenter(
  screen: Awaited<ReturnType<SkyEngineE2e["screen"]>>,
): { x: number; y: number } | undefined {
  const positions: Array<{ x: number; y: number }> = [];
  for (let y = 30; y < 280; y += 1) {
    for (let x = 0; x < screen.width; x += 1) {
      if (screen.pixel(x, y).join(",") === "0,252,0") positions.push({ x, y });
    }
  }
  if (positions.length === 0) return undefined;
  return {
    x: positions.reduce((sum, position) => sum + position.x, 0) / positions.length,
    y: positions.reduce((sum, position) => sum + position.y, 0) / positions.length,
  };
}

describe("gtdgdq wall collision", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("keeps running after the launched ball hits a wall with sound enabled", async () => {
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path("mythroad/gtdgdq"), { recursive: true, force: true });
    engine = await SkyEngineE2e.start("test/fixtures/gtdgdq.mrp", { workDir: ws.dir });

    await engine.waitForPixel(219, 312, [0, 200, 248], {
      name: "wall-bgm-select",
      timeoutMs: 10_000,
      intervalMs: 250,
    });
    await engine.key("LEFT_SOFT", 1_000);
    await engine.waitForPixel(168, 162, [248, 248, 240], {
      name: "wall-menu",
      timeoutMs: 10_000,
      intervalMs: 250,
    });

    await engine.key("LEFT_SOFT", 1_000);
    await engine.waitForScreen(screen => screen.uniqueColorCount() === 2, {
      name: "wall-motion-prompt",
      timeoutMs: 10_000,
      intervalMs: 250,
    });
    await engine.key("RIGHT_SOFT", 1_000);
    await engine.key("DOWN", 1_000);
    await engine.key("LEFT_SOFT", 1_000);
    await engine.waitForPixel(120, 160, [152, 40, 176], {
      name: "wall-level-one-prompt",
      timeoutMs: 10_000,
      intervalMs: 250,
    });
    await engine.key("SELECT", 1_000);
    const readyToLaunch = await engine.waitForScreen(isGameScreen, {
      name: "wall-ready-to-launch",
      timeoutMs: 10_000,
      intervalMs: 100,
    });
    const readyBall = ballCenter(readyToLaunch);
    expect(readyBall).toBeDefined();

    await engine.key("SELECT", 1_000);
    await engine.delay(5_000);

    expect(await engine.waitForExit(250)).toBe(false);
    const afterCollision = await engine.screen("wall-after-collision");
    const movingBall = ballCenter(afterCollision);
    expect(isGameScreen(afterCollision)).toBe(true);
    expect(movingBall).toBeDefined();
    expect(movingBall).not.toEqual(readyBall);
    expect(afterCollision.diffPixelCount(readyToLaunch)).toBeGreaterThan(0);
  });
});
