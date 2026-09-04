import fs from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import {
  simpleDownloadDnsMap,
  simpleDownloadRequestAppId,
  startSimpleDownloadServer,
  type SimpleDownloadServer,
} from "../simple-download-server.js";
import { hasSoftwareUpdateHeader, isPluginPrompt } from "./visual.js";

const GAME_REGION = { x: 0, y: 40, width: 240, height: 280 };
const ADVBAR_REGION = { x: 0, y: 0, width: 240, height: 40 };

describe("optwar", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  let downloadServer: SimpleDownloadServer | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await downloadServer?.close();
    downloadServer = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("令牌插件下载完成后完整恢复支付方式画面", async () => {
    ws = await SkyEngineWorkspace.create();
    for (const plugin of ["netpay", "advbar"]) {
      const target = ws.path(`mythroad/plugins/${plugin}.mrp`);
      if (!fs.existsSync(target)) {
        fs.cpSync(`test/fixtures/plugins/${plugin}.mrp`, target);
      }
    }
    const freecurrPath = ws.path("mythroad/plugins/freecurr.mrp");
    const freecurrPlugin = fs.readFileSync("test/fixtures/plugins/freecurr.mrp");
    fs.rmSync(freecurrPath, { force: true });
    downloadServer = await startSimpleDownloadServer(freecurrPlugin);
    engine = await SkyEngineE2e.start("test/fixtures/optwar.mrp", {
      workDir: ws.dir,
      dnsMap: simpleDownloadDnsMap(downloadServer),
    });

    await engine.waitForScreen(
      screen => screen.pixel(227, 301).toString() === "248,0,0",
      { name: "bgm-select", timeoutMs: 60_000, intervalMs: 250 },
    );
    await engine.click(227, 301, 1_000);
    await engine.waitForScreen(
      screen => screen.pixel(98, 264).toString() === "0,252,0",
      { name: "menu", timeoutMs: 10_000, intervalMs: 100 },
    );

    // The foreground advbar consumes the first key before the game starts.
    await engine.key("RIGHT", 1_000);
    await engine.key("ENTER", 1_000);
    await engine.delay(1_000);
    await engine.key("ENTER", 1_000);
    await engine.delay(1_000);
    await engine.key("ENTER", 1_000);
    await engine.waitForScreen(
      screen => screen.pixel(22, 314).toString() === "200,252,248",
      { name: "game-started", timeoutMs: 10_000, intervalMs: 100 },
    );

    await engine.key("LEFT_SOFT", 1_000);
    await engine.waitForScreen(
      screen => screen.pixel(175, 103).toString() === "48,188,248",
      { name: "game-menu", timeoutMs: 10_000, intervalMs: 100 },
    );
    await engine.key("ENTER", 1_000);
    await engine.waitForScreen(
      screen => screen.pixel(213, 151).toString() === "200,252,248",
      { name: "full-power", timeoutMs: 10_000, intervalMs: 100 },
    );
    await engine.key("LEFT_SOFT", 1_000);
    await engine.waitForScreen(
      screen => screen.pixel(230, 269).toString() === "48,188,248"
        && screen.pixel(230, 20).toString() === "168,20,32",
      { name: "payment-method", timeoutMs: 3_000, intervalMs: 100 },
    );
    await engine.key("DOWN", 1_000);
    const tokenPayment = await engine.waitForScreen(
      screen => screen.pixel(222, 287).toString() === "48,188,248"
        && screen.pixel(230, 20).toString() === "168,20,32",
      { name: "token-payment", timeoutMs: 3_000, intervalMs: 100 },
    );

    await engine.key("ENTER", { waitForDraw: false });
    await engine.waitForScreen(isPluginPrompt, {
      name: "freecurr-download-prompt",
      timeoutMs: 3_000,
      intervalMs: 100,
    });
    await engine.key("LEFT_SOFT", 1_000);
    const installedScreen = await engine.waitForScreen(
      screen => fs.existsSync(freecurrPath)
        && screen.pixel(120, 100).toString() === "232,240,248"
        && screen.pixel(120, 310).toString() === "0,132,208",
      { name: "freecurr-installed", timeoutMs: 30_000, intervalMs: 250 },
    );
    const installedPlugin = fs.readFileSync(freecurrPath);
    expect(installedPlugin.equals(freecurrPlugin)).toBe(true);
    expect(installedPlugin.readUInt32LE(0x44)).toBe(490327);
    expect(installedPlugin.readUInt32LE(0x48)).toBe(1011);
    expect(downloadServer.requests.length).toBeGreaterThan(0);
    expect(downloadServer.requests.some(request => simpleDownloadRequestAppId(request) === 490327))
      .toBe(true);
    expect(hasSoftwareUpdateHeader(installedScreen)).toBe(true);

    await engine.key("LEFT_SOFT", 3_000);
    const paymentReturned = await engine.waitForScreen(
      screen => screen.diffPixelCount(tokenPayment, GAME_REGION) === 0,
      { name: "payment-after-freecurr-install", timeoutMs: 10_000, intervalMs: 100 },
    );
    expect(hasSoftwareUpdateHeader(paymentReturned)).toBe(false);
    expect(paymentReturned.diffPixelCount(tokenPayment, ADVBAR_REGION)).toBe(0);
    expect(paymentReturned.diffPixelCount(tokenPayment)).toBe(0);
  });
});
