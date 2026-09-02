import { createHash } from "node:crypto";
import { copyFile, readFile } from "node:fs/promises";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage } from "../engine-e2e.js";

const MRP_SHA256 = "611b4cd737dcf458370ff215bc73636cf65bdc6dfc36907c2ce8aa8f00b7c8e2";
const LOCAL_DNS_MAP = [
  "wap.skmeg.com->127.0.0.1",
  "rop.skymobiapp.com->127.0.0.1",
  "spd.skymobiapp.com->127.0.0.1",
  "freeads.51mrp.com->127.0.0.1",
  "proxy.51mrp.com->127.0.0.1",
  "proxy2.51mrp.com->127.0.0.1",
  "help.proxy.51mrp.com->127.0.0.1",
].join(";");

async function sha256(file: string): Promise<string> {
  return createHash("sha256").update(await readFile(file)).digest("hex");
}

function isGameplayFrame(screen: PpmImage): boolean {
  const [red, green, blue] = screen.pixel(20, 250);
  return red === 96 && green === 64 && blue === 0
    && screen.uniqueColorCount() > 1_000;
}

function hasRenderedScore(screen: PpmImage): boolean {
  const [red, green, blue] = screen.pixel(58, 8);
  return red === 248 && green === 252 && blue === 0;
}

const SCORE_RECT = { x: 50, y: 4, width: 28, height: 20 } as const;

function scoreDigitPixelCount(screen: PpmImage): number {
  let count = 0;
  for (let y = SCORE_RECT.y; y < SCORE_RECT.y + SCORE_RECT.height; y++) {
    for (let x = SCORE_RECT.x; x < SCORE_RECT.x + SCORE_RECT.width; x++) {
      const [red, green, blue] = screen.pixel(x, y);
      if (red === 248 && green === 252 && blue === 0) count++;
    }
  }
  return count;
}

describe("op6120 gameplay", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("keeps running after a fired projectile collides with an enemy", async () => {
    expect(await sha256("test/fixtures/op6120.mrp")).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    const mrp = ws.path("mythroad/op6120.mrp");
    await copyFile("test/fixtures/op6120.mrp", mrp);
    await copyFile("test/fixtures/plugins/advbar.mrp", ws.path("mythroad/plugins/advbar.mrp"));

    engine = await SkyEngineE2e.start(mrp, {
      workDir: ws.dir,
      memory: "4M",
      dnsMap: LOCAL_DNS_MAP,
      timeoutMs: 90_000,
    });
    await engine.waitForScreen(
      screen => screen.pixel(184, 128).every(channel => channel === 240)
        && screen.uniqueColorCount() === 1_697,
      { name: "title", timeoutMs: 90_000, intervalMs: 250 },
    );

    await engine.key("ENTER", { holdMs: 1, timeoutMs: 5_000 });
    const gameplayStarted = await engine.waitForScreen(
      screen => isGameplayFrame(screen) && hasRenderedScore(screen),
      {
        name: "gameplay-started",
        timeoutMs: 10_000,
        intervalMs: 50,
      },
    );
    const initialScorePixels = scoreDigitPixelCount(gameplayStarted);
    expect(initialScorePixels).toBe(22);
    const gameplayDrawCount = await engine.drawCount();

    // Aim across the enemy lanes. Pointer presses launch projectiles; ENTER is
    // only used by the title screen and does not fire during gameplay. A spread
    // of trajectories makes the collision independent of a wave's exact phase.
    const targets = [
      [200, 200], [160, 190], [120, 180], [80, 170], [40, 160],
      [200, 140], [160, 140], [120, 140], [80, 140], [40, 140],
    ] as const;
    let afterCollision: PpmImage | undefined;
    for (let shot = 0; shot < 120; shot++) {
      const [x, y] = targets[shot % targets.length];
      await engine.click(x, y, 5_000);
      if ((shot + 1) % 5 === 0) {
        const screen = await engine.screen("after-enemy-collision");
        if (hasRenderedScore(screen)
          && screen.uniqueColorCount() > 1_000
          && scoreDigitPixelCount(screen) > initialScorePixels + 10
          && gameplayStarted.diffPixelCount(screen, SCORE_RECT) > 15) {
          afterCollision = screen;
          break;
        }
      }
    }

    expect(afterCollision, "enemy collision did not update the score").toBeDefined();
    if (!afterCollision) throw new Error("enemy collision frame was not captured");
    expect(afterCollision.uniqueColorCount()).toBeGreaterThan(1_000);
    expect(scoreDigitPixelCount(afterCollision)).toBeGreaterThan(initialScorePixels + 10);
    expect(gameplayStarted.diffPixelCount(afterCollision, SCORE_RECT)).toBeGreaterThan(15);
    const collisionDrawCount = await engine.drawCount();
    expect(collisionDrawCount).toBeGreaterThan(gameplayDrawCount + 20);
    await vi.waitFor(async () => {
      expect(await engine!.drawCount()).toBeGreaterThan(collisionDrawCount);
    }, { timeout: 5_000, interval: 100 });

    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(/unsupported platform|ABI error|MR fault|FATAL/);
  }, 120_000);
});
