import { createHash } from "node:crypto";
import { copyFile, readFile } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
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
    const gameplayDrawCount = await engine.drawCount();

    // Projectiles travel up the launcher's fixed lane. Repeated shots span a
    // full enemy wave and deterministically exercise the collision sound path.
    let afterCollision: PpmImage | undefined;
    for (let shot = 0; shot < 260; shot++) {
      await engine.key("ENTER", { holdMs: 1, waitForDraw: false });
      if ((shot + 1) % 10 === 0) {
        const screen = await engine.screen("after-enemy-collision");
        if (isGameplayFrame(screen)
          && hasRenderedScore(screen)
          && gameplayStarted.diffPixelCount(screen, { x: 50, y: 4, width: 28, height: 20 }) > 15) {
          afterCollision = screen;
          break;
        }
      }
    }

    expect(afterCollision, "enemy collision did not update the score").toBeDefined();
    if (!afterCollision) throw new Error("enemy collision frame was not captured");
    expect(afterCollision.pixel(20, 250)).toEqual([96, 64, 0]);
    expect(gameplayStarted.diffPixelCount(
      afterCollision,
      { x: 50, y: 4, width: 28, height: 20 },
    )).toBeGreaterThan(15);
    expect(await engine.drawCount()).toBeGreaterThan(gameplayDrawCount + 100);

    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(/unsupported platform|ABI error|MR fault|FATAL/);
  }, 120_000);
});
