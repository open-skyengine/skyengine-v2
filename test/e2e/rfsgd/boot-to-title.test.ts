import { createHash } from "node:crypto";
import { readFile, stat } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const MRP_SHA256 = "d1e27c69c344343281209f027d47aa0ebc95b693a587e1bc492ebb23949d66e4";
const PAK_SHA256 = "6d88f03003726563e873c58a2bd3c5b645b55588c07f97c68a932f5663840fd4";

describe("rfsgd 启动", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("首次展开分块资源并进入标题界面", async () => {
    const mrp = "test/fixtures/rfsgd_220x176.mrp";
    expect(createHash("sha256").update(await readFile(mrp)).digest("hex")).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    const pak = ws.path("mythroad/rfsgd/gfszgdyx.pak");
    await expect(stat(pak)).rejects.toThrow();
    engine = await SkyEngineE2e.start(mrp, {
      workDir: ws.dir,
      screen: "220x176",
      timeoutMs: 120_000,
    });
    const musicPrompt = await engine.waitForScreen(
      screen => screen.width === 220
        && screen.height === 176
        && screen.uniqueColorCount() === 19
        && screen.pixel(15, 160).toString() === "248,252,248"
        && screen.pixel(205, 160).toString() === "248,252,248",
      { name: "music-prompt", timeoutMs: 120_000, intervalMs: 250 },
    );
    expect(musicPrompt.pixel(110, 88)).toEqual([16, 236, 232]);

    await engine.key("SOFTRIGHT", { timeoutMs: 10_000 });
    const title = await engine.waitForScreen(
      screen => screen.width === 220
        && screen.height === 176
        && screen.uniqueColorCount() === 198
        && screen.pixel(20, 20).toString() === "120,96,40"
        && screen.pixel(110, 150).toString() === "8,16,32",
      { name: "title", timeoutMs: 60_000, intervalMs: 250 },
    );

    expect(title.pixel(110, 88)).toEqual([8, 8, 8]);
    expect(title.pixel(200, 160)).toEqual([8, 16, 32]);
    const expandedPak = await readFile(pak);
    expect(expandedPak.length).toBe(724_879);
    expect(createHash("sha256").update(expandedPak).digest("hex")).toBe(PAK_SHA256);
    expect(await engine.waitForExit(1_000)).toBe(false);
    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(/ARM fault|ABI error|MR fault|unmapped|panicked at/i);
  }, 150_000);
});
