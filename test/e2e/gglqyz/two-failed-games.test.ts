import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const MRP_SHA256 = "9b712d8709c88b1e8c02a8c72ce10986b8f686fa4843dea8515d791a3e4906cd";

describe("gglqyz 连续游戏失败", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("连续失败两局后仍保持运行", async () => {
    const mrp = "test/fixtures/gglqyz.mrp";
    expect(createHash("sha256").update(await readFile(mrp)).digest("hex")).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start(mrp, {
      workDir: ws.dir,
      timeoutMs: 90_000,
    });

    await engine.waitForScreen(
      screen => screen.uniqueColorCount() === 2
        && screen.pixel(120, 158).toString() === "248,252,248",
      { name: "01-sound-prompt", timeoutMs: 90_000, intervalMs: 250 },
    );
    await engine.key("SOFTRIGHT", { timeoutMs: 10_000 });
    const titlePredicate = (screen: Awaited<ReturnType<SkyEngineE2e["screen"]>>) =>
      screen.uniqueColorCount() > 100
        && screen.pixel(20, 50).toString() === "232,240,184"
        && screen.pixel(200, 20).toString() === "16,40,48";
    const tutorialPredicate = (screen: Awaited<ReturnType<SkyEngineE2e["screen"]>>) =>
      screen.uniqueColorCount() < 20
        && screen.pixel(1, 1).toString() === "0,252,248"
        && screen.pixel(20, 306).toString() === "24,140,0"
        && screen.pixel(214, 306).toString() === "248,144,112";
    const modePredicate = (screen: Awaited<ReturnType<SkyEngineE2e["screen"]>>) =>
      screen.pixel(120, 15).toString() === "248,236,24"
        && screen.pixel(120, 64).toString() === "200,116,0"
        && screen.pixel(120, 104).toString() === "0,108,104";

    await engine.waitForScreen(
      titlePredicate,
      { name: "02-title", timeoutMs: 30_000, intervalMs: 250 },
    );

    const loseGame = async (round: number) => {
      await engine!.key("ENTER", { timeoutMs: 30_000 });
      await engine!.waitForScreen(
        tutorialPredicate,
        { name: `${round}-tutorial-prompt`, timeoutMs: 10_000, intervalMs: 250 },
      );
      await engine!.key("SOFTRIGHT", { timeoutMs: 30_000 });
      await engine!.waitForScreen(
        modePredicate,
        { name: `${round}-mode-selection`, timeoutMs: 10_000, intervalMs: 250 },
      );
      await engine!.key("DOWN", { timeoutMs: 10_000 });
      await engine!.key("ENTER", { timeoutMs: 30_000 });
      await engine!.delay(1_000);
      await engine!.screen(`${round}-character-selection`);
      await engine!.key("ENTER", { timeoutMs: 30_000 });
      await engine!.delay(1_000);
      await engine!.screen(`${round}-opponent-selection`);
      await engine!.key("ENTER", { timeoutMs: 30_000 });
      await engine!.waitForScreen(
        screen => screen.pixel(0, 0).toString() === "24,32,40"
          && screen.pixel(120, 300).toString() === "0,108,104",
        { name: `${round}-shop`, timeoutMs: 15_000, intervalMs: 250 },
      );
      await engine!.key("SOFTLEFT", { timeoutMs: 30_000 });
      await engine!.waitForScreen(
        screen => screen.uniqueColorCount() > 300
          && screen.pixel(0, 0).toString() === "248,252,248"
          && screen.pixel(120, 300).toString() === "72,0,0",
        { name: `${round}-battle`, timeoutMs: 10_000, intervalMs: 250 },
      );
      await engine!.waitForScreen(
        titlePredicate,
        { name: `${round}-failed-title`, timeoutMs: 60_000, intervalMs: 500 },
      );
      expect(await engine!.waitForExit(250)).toBe(false);
    };

    await loseGame(1);
    await loseGame(2);

    await engine.key("ENTER", { timeoutMs: 30_000 });
    await engine.waitForScreen(
      tutorialPredicate,
      { name: "3-after-two-failures", timeoutMs: 10_000, intervalMs: 250 },
    );

    expect(await engine.waitForExit(250)).toBe(false);
    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(/ARM fault|ABI error|MR fault|unmapped|panicked at/i);
  }, 180_000);
});
