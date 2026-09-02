import { afterEach, describe, expect, it, vi } from "vitest";
import { cpSync } from "node:fs";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const CACHED_CHANNEL_SMS = Buffer.from(
  "000003f1000000093030303030303030360000044f000000040000000000" +
  "00000000000000000000000c120000ae10000000000000111100002b1100" +
  "000000000000000000",
  "hex",
);

const CACHED_COMBO_SID = Buffer.from(
  "71570c6b3c887df241613e553e887cf041670c6529fe4cc071530c601e5d4cc0" +
  "56100c650ebc4cc0719f0c6529f04cc071530c650eb84cc0561e0c650eb778f6" +
  "41673d5c39887bf343603f552e9e2ec97e06cb7aa49e2eee3d06cb5de39e2efc" +
  "4f3571a3bb122771013571a3eb1227712e",
  "hex",
);

describe("gzwdzjs 商城", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("复用旧 channel 状态购买充足阳光后正常退出游戏", async () => {
    ws = await SkyEngineWorkspace.create();
    await rm(ws.path("mythroad/6110cookie.inf"), { force: true });
    await rm(ws.path("mythroad/app240320/cfg/localdir.sav"), { force: true });
    await writeFile(ws.path("mythroad/time.py.sys"), "0000000029##");
    await mkdir(ws.path("mythroad/system/ntp"), { recursive: true });
    await writeFile(ws.path("mythroad/system/ntp/combo.sid"), CACHED_COMBO_SID);
    const app = ws.path("mythroad/gzwdzjs.mrp");
    cpSync("test/fixtures/gzwdzjs.mrp", app, { preserveTimestamps: true });
    await mkdir(ws.path("mythroad/gzwdzjs"), { recursive: true });
    const channel = ws.path("mythroad/gzwdzjs/channel.sms");
    await writeFile(channel, CACHED_CHANNEL_SMS);
    // A cache newer than the installed package is reused instead of regenerated.
    expect((await stat(channel)).mtimeMs).toBeGreaterThan((await stat(app)).mtimeMs);
    engine = await SkyEngineE2e.start(app, {
      workDir: ws.dir,
    });

    await engine.delay(5_000);
    await engine.key("RIGHT_SOFT", 1_000);
    await engine.waitForPixel(169, 117, [232, 176, 152], {
      name: "shop-main-menu-initial",
      timeoutMs: 30_000,
    });

    for (let i = 0; i < 3; i++) {
      await engine.key("ENTER", 1_000);
      await engine.delay(1_000);
    }
    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const screen = await engine.screen("shop-story-a");
      expect(screen.pixel(94, 145)).toEqual([208, 244, 200]);
    }, { timeout: 30_000, interval: 1_000 });

    for (let i = 0; i < 3; i++) {
      await engine.key("ENTER", 1_000);
      await engine.delay(1_000);
    }
    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const screen = await engine.screen("shop-story-b");
      expect(screen.pixel(94, 145)).toEqual([200, 204, 248]);
    }, { timeout: 30_000, interval: 1_000 });

    await engine.key("ENTER", 1_000);
    await engine.delay(1_000);
    await engine.key("LEFT_SOFT", 1_000);
    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const screen = await engine.screen("shop-after-skip");
      expect(screen.pixel(94, 145)).toEqual([208, 244, 200]);
    }, { timeout: 90_000, interval: 1_000 });

    for (let i = 0; i < 5; i++) {
      await engine.key("ENTER", 1_000);
      await engine.delay(1_000);
    }
    await engine.key("LEFT_SOFT", 1_000);
    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const screen = await engine.screen("shop-tutorial-intro");
      expect(screen.pixel(94, 145)).toEqual([208, 244, 200]);
    }, { timeout: 90_000, interval: 1_000 });

    for (let i = 0; i < 2; i++) {
      await engine.key("ENTER", 1_000);
      await engine.delay(1_000);
    }
    const game = await engine.screen("shop-game");
    expect(game.pixel(42, 245)).toEqual([24, 12, 0]);

    await engine.key("RIGHT_SOFT", 5_000);
    await engine.delay(2_000);
    const shop = await engine.screen("shop");
    expect(shop.pixel(1, 1)).toEqual([184, 252, 0]);
    expect(shop.pixel(10, 10)).toEqual([208, 244, 200]);

    await engine.key("ENTER", 5_000);
    await engine.delay(2_000);
    const purchase = await engine.screen("shop-purchase");
    expect(purchase.pixel(100, 100)).toEqual([104, 104, 224]);
    expect(purchase.pixel(100, 300)).toEqual([0, 104, 208]);

    await engine.key("ENTER", 5_000);
    await engine.delay(2_000);
    const result = await engine.screen("shop-result");
    expect(result.pixel(100, 100)).toEqual([104, 104, 224]);
    expect(purchase.diffPixelCount(result)).toBeGreaterThan(2_000);

    await engine.delay(5_000);
    const returned = await engine.screen("shop-returned");
    expect(returned.pixel(1, 1)).toEqual([184, 252, 0]);
    expect(returned.pixel(10, 10)).toEqual([208, 244, 200]);
    expect(returned.diffPixelCount(shop)).toBe(0);

    await engine.key("RIGHT_SOFT", 5_000);
    await engine.delay(2_000);
    const exitedShop = await engine.screen("shop-exited");
    expect(exitedShop.pixel(42, 245)).toEqual([24, 12, 0]);

    await engine.key("LEFT_SOFT", 5_000);
    await engine.delay(2_000);
    const paused = await engine.screen("shop-paused");
    expect(paused.pixel(1, 1)).toEqual([184, 252, 0]);
    expect(paused.pixel(10, 10)).toEqual([208, 244, 200]);

    for (let i = 0; i < 5; i++) {
      await engine.key("DOWN", 1_000);
    }
    await engine.screen("shop-return-to-menu-selected");
    await engine.key("ENTER", 5_000);
    await engine.delay(2_000);
    const returnConfirmation = await engine.screen("shop-after-return-to-menu");
    expect(returnConfirmation.pixel(1, 1)).toEqual([184, 252, 0]);
    expect(returnConfirmation.pixel(169, 117)).toEqual([0, 80, 24]);

    await engine.key("LEFT_SOFT", 5_000);
    await engine.delay(2_000);
    const mainMenu = await engine.screen("shop-main-menu");
    expect(mainMenu.pixel(169, 117)).toEqual([232, 176, 152]);
    expect(mainMenu.pixel(38, 22)).toEqual([152, 228, 0]);

    for (let i = 0; i < 5; i++) {
      await engine.key("RIGHT", 1_000);
      await engine.delay(250);
    }
    const exitSelected = await engine.screen("shop-exit-selected");
    expect(exitSelected.pixel(169, 117)).toEqual([232, 176, 152]);
    expect(exitSelected.diffPixelCount(mainMenu)).toBeGreaterThan(100);
    await engine.key("ENTER", { holdMs: 80, waitForDraw: false });
    await engine.delay(2_000);
    const exitConfirmation = await engine.screen("shop-after-exit-command");
    expect(exitConfirmation.pixel(100, 100)).toEqual([0, 0, 0]);
    expect(exitConfirmation.pixel(7, 312)).toEqual([0, 252, 24]);
    expect(exitConfirmation.pixel(231, 312)).toEqual([0, 252, 24]);
    await engine.key("LEFT_SOFT", { holdMs: 80, waitForDraw: false });
    expect(await engine.waitForExit(5_000)).toBe(true);

    await engine.stop();
    const stderr = await readFile(engine.stderrPath, "utf8");
    expect(stderr).not.toMatch(/(?:MR|ARM) fault/);
  });
});
