import { createHash } from "node:crypto";
import { readFile, stat } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const MRP_SHA256 = "2282a7f0f57e41405bf9a1c7fb3c60ef0f47ba9257f8bd8e78fb6fb555e92e96";
const PAK_SHA256 = "21fef3837a4b2718f1b89b5944972e7fce164a5539b8918bfb9ee67f32b79e50";

async function sha256(file: string): Promise<string> {
  return createHash("sha256").update(await readFile(file)).digest("hex");
}

describe("gzsyr 冷启动", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("首次展开分块资源并进入标题界面", async () => {
    const mrp = "test/fixtures/gzsyr.mrp";
    expect(await sha256(mrp)).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    const pak = ws.path("mythroad/gzsyr/res.pak");
    await expect(stat(pak)).rejects.toThrow();

    // Deliberately omit --memory: this package must boot with the normal profile.
    engine = await SkyEngineE2e.start(mrp, {
      workDir: ws.dir,
      timeoutMs: 120_000,
    });
    const musicPrompt = await engine.waitForScreen(
      screen => screen.width === 240
        && screen.height === 320
        && screen.uniqueColorCount() === 36
        && screen.pixel(120, 100).toString() === "248,176,40"
        && screen.pixel(120, 160).toString() === "112,0,160"
        && screen.pixel(20, 275).toString() === "0,80,8"
        && screen.pixel(215, 275).toString() === "112,28,0",
      { name: "music-prompt", timeoutMs: 120_000, intervalMs: 250 },
    );
    expect(musicPrompt.pixel(0, 0)).toEqual([56, 8, 0]);
    expect(musicPrompt.pixel(239, 319)).toEqual([56, 8, 0]);

    await engine.key("SOFTRIGHT", { timeoutMs: 10_000, waitForDraw: false });
    const title = await engine.waitForScreen(
      screen => screen.width === 240
        && screen.height === 320
        && screen.uniqueColorCount() === 137
        && screen.pixel(20, 20).toString() === "88,40,24"
        && screen.pixel(120, 100).toString() === "8,8,16"
        && screen.pixel(120, 160).toString() === "248,252,248"
        && screen.pixel(215, 275).toString() === "248,44,0",
      { name: "title", timeoutMs: 60_000, intervalMs: 250 },
    );
    expect(title.pixel(0, 0)).toEqual([176, 144, 128]);
    expect(title.diffPixelCount(musicPrompt)).toBeGreaterThan(70_000);

    expect((await stat(pak)).size).toBe(799_957);
    expect(await sha256(pak)).toBe(PAK_SHA256);
    expect(await engine.waitForExit(1_000)).toBe(false);

    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(
      /ARM fault|ABI error|MR fault|unmapped|instruction budget|guest heap exhausted|no memory|panic/i,
    );
  }, 180_000);
});
