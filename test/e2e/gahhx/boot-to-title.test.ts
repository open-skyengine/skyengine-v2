import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

describe("gahhx startup", () => {
  let engine: SkyEngineE2e | undefined;
  let workspace: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await workspace?.dispose();
    workspace = undefined;
  });

  it("loads the MTK game image and presents its first screen", async () => {
    workspace = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/e2e/fixtures/gahhx_v1005.mrp", {
      workDir: workspace.dir,
      timeoutMs: 60_000
    });

    const screen = await engine.waitForScreen(image => image.uniqueColorCount() > 50, {
      name: "title",
      timeoutMs: 90_000,
      intervalMs: 500
    });

    expect(screen.width).toBe(240);
    expect(screen.height).toBe(320);
    expect(screen.pixel(120, 160)).toEqual([184, 232, 248]);
    expect(screen.pixel(239, 319)).toEqual([32, 128, 192]);
    expect(await engine.drawCount()).toBeGreaterThan(0);
    await engine.stop();
    expect(await readFile(engine.stderrPath, "utf8")).not.toContain("MR fault");
  });
});
