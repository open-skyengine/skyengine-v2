import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const MRP_SHA256 = "9b712d8709c88b1e8c02a8c72ce10986b8f686fa4843dea8515d791a3e4906cd";

describe("gglqyz 启动", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("通过声音询问页进入标题界面且保持运行", async () => {
    const mrp = "test/fixtures/gglqyz.mrp";
    expect(createHash("sha256").update(await readFile(mrp)).digest("hex")).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start(mrp, {
      workDir: ws.dir,
      timeoutMs: 90_000,
    });
    const soundPrompt = await engine.waitForScreen(
      screen => screen.uniqueColorCount() === 2
        && screen.pixel(120, 158).toString() === "248,252,248",
      { name: "sound-prompt", timeoutMs: 90_000, intervalMs: 250 },
    );
    expect(soundPrompt.pixel(120, 158)).toEqual([248, 252, 248]);
    expect(soundPrompt.pixel(223, 304)).toEqual([248, 252, 248]);

    await engine.key("SOFTRIGHT", { timeoutMs: 10_000 });
    const title = await engine.waitForScreen(
      screen => screen.uniqueColorCount() > 100
        && screen.pixel(20, 50).toString() === "232,240,184"
        && screen.pixel(200, 20).toString() === "16,40,48"
        && screen.pixel(200, 150).toString() === "200,196,200",
      { name: "title", timeoutMs: 30_000, intervalMs: 250 },
    );

    expect(title.width).toBe(240);
    expect(title.height).toBe(320);
    expect(title.pixel(120, 300)).toEqual([8, 20, 24]);
    expect(await engine.waitForExit(250)).toBe(false);
    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(/ARM fault|ABI error|MR fault|unmapped|panicked at/i);
  }, 120_000);
});
