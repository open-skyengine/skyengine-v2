import { afterEach, describe, expect, it } from "vitest";
import fs from "node:fs";
import type { PpmImage } from "../engine-e2e.js";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import { isPluginPrompt } from "./visual.js";

const GAME_REGION = { x: 0, y: 40, width: 240, height: 280 };

interface VisualRateSample {
  drawRate: number;
  visualRate: number;
  changedPixels: number;
  first: PpmImage;
  last: PpmImage;
}

async function captureVisualRate(
  engine: SkyEngineE2e,
  name: string,
  durationMs = 1_500,
): Promise<VisualRateSample> {
  const firstDraw = await engine.drawCount();
  const startedAt = performance.now();
  await engine.delay(durationMs);
  const lastDraw = await engine.drawCount();
  const elapsedSeconds = (performance.now() - startedAt) / 1_000;
  const drawCount = lastDraw - firstDraw;
  if (drawCount < 1 || drawCount >= 128) {
    throw new Error(`visual sample retained ${drawCount} frames; expected 1..127`);
  }

  let previous = await engine.screenDraw(firstDraw, `${name}-first`);
  const first = previous;
  let visuallyChangedFrames = 0;
  let changedPixels = 0;
  for (let draw = firstDraw + 1; draw <= lastDraw; draw += 1) {
    const current = await engine.screenDraw(draw, `${name}-${draw}`);
    const difference = current.diffPixelCount(previous, GAME_REGION);
    if (difference > 100) visuallyChangedFrames += 1;
    changedPixels += difference;
    previous = current;
  }

  return {
    drawRate: drawCount / elapsedSeconds,
    visualRate: visuallyChangedFrames / elapsedSeconds,
    changedPixels,
    first,
    last: previous,
  };
}

describe("optwar", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("点击火力全开广告进入浏览器并返回后画面速度保持稳定", async () => {
    ws = await SkyEngineWorkspace.create();
    if (!fs.existsSync(ws.path("mythroad/plugins/advbar.mrp"))) {
      fs.cpSync(
        "test/fixtures/plugins/advbar.mrp",
        ws.path("mythroad/plugins/advbar.mrp"),
      );
    }
    fs.rmSync(ws.path("mythroad/brw"), { recursive: true, force: true });
    fs.rmSync(ws.path("mythroad/plugins/embrw.mrp"), { force: true });
    fs.rmSync(ws.path("mythroad/plugins/brwcore.mrp"), { force: true });
    fs.rmSync(ws.path("mythroad/plugins/brwgui.mrp"), { force: true });
    fs.rmSync(ws.path("mythroad/plugins/brwmain.mrp"), { force: true });
    fs.rmSync(ws.path("mythroad/plugins/brwshell.mrp"), { force: true });
    fs.rmSync(ws.path("mythroad/plugins/dump0"), { force: true });
    engine = await SkyEngineE2e.start("test/fixtures/optwar.mrp", {
      workDir: ws.dir,
      dnsMap: "10.0.0.172->159.75.119.124;rop.skymobiapp.com->159.75.119.124;spd.skymobiapp.com->159.75.119.124;proxy.51mrp.com->159.75.119.124;proxy2.51mrp.com->159.75.119.124",
    });

    const boot = await engine.waitForScreen(
      screen => screen.pixel(227, 301).toString() === "248,0,0",
      { name: "bgm-select", timeoutMs: 60_000, intervalMs: 250 },
    );
    expect(boot.pixel(150, 308)).toEqual([0, 0, 0]);
    await engine.click(227, 301, 1_000);

    const menu = await engine.waitForScreen(
      screen => screen.pixel(98, 264).toString() === "0,252,0",
      { name: "menu", timeoutMs: 10_000, intervalMs: 100 },
    );
    expect(menu.pixel(110, 27)).toEqual([128, 48, 40]);

    // 第一次方向键只关闭覆盖在主菜单顶部的前台广告条。
    await engine.key("RIGHT", 1_000);
    const menuWithoutAd = await engine.waitForScreen(
      screen => screen.pixel(110, 27).toString() !== "128,48,40"
        && screen.pixel(98, 264).toString() === "0,252,0",
      { name: "menu-without-ad", timeoutMs: 3_000, intervalMs: 100 },
    );
    expect(menuWithoutAd.diffPixelCount(menu)).toBeGreaterThan(0);

    await engine.key("ENTER", 1_000);
    await engine.delay(1_000);
    await engine.key("ENTER", 1_000);
    await engine.delay(1_000);
    await engine.key("ENTER", 1_000);
    const gameBeforeAd = await engine.waitForScreen(
      screen => screen.pixel(22, 314).toString() === "200,252,248",
      { name: "game-before-ad", timeoutMs: 10_000, intervalMs: 100 },
    );

    const baseline = await captureVisualRate(engine, "baseline");
    expect(baseline.visualRate).toBeGreaterThan(5);
    expect(baseline.last.diffPixelCount(baseline.first, GAME_REGION)).toBeGreaterThan(1_000);

    await engine.key("LEFT_SOFT", 1_000);
    const gameMenu = await engine.waitForScreen(
      screen => screen.pixel(175, 103).toString() === "48,188,248",
      { name: "game-menu", timeoutMs: 10_000, intervalMs: 100 },
    );
    expect(gameMenu.diffPixelCount(gameBeforeAd)).toBeGreaterThan(0);

    await engine.key("ENTER", 1_000);
    const fullPower = await engine.waitForScreen(
      screen => screen.pixel(213, 151).toString() === "200,252,248",
      { name: "full-power", timeoutMs: 10_000, intervalMs: 100 },
    );

    // “火力全开”详情页当前商品的上一项就是广告条。
    await engine.key("UP", 1_000);
    const adSelected = await engine.waitForScreen(
      screen => screen.pixel(0, 0).toString() === "104,184,224"
        && screen.pixel(0, 0).toString() !== fullPower.pixel(0, 0).toString(),
      { name: "ad-selected", timeoutMs: 3_000, intervalMs: 100 },
    );
    expect(adSelected.diffPixelCount(fullPower)).toBeGreaterThan(0);

    await engine.key("ENTER", { waitForDraw: false });
    const browserDownload = await engine.waitForScreen(isPluginPrompt, {
      name: "browser-plugin-download",
      timeoutMs: 3_000,
      intervalMs: 100,
    });
    expect(isPluginPrompt(browserDownload)).toBe(true);

    await engine.key("LEFT_SOFT", 1_000);
    const browserPluginInstalled = await engine.waitForScreen(
      screen => screen.pixel(0, 0).toString() === "56,140,192"
        && screen.pixel(120, 100).toString() === "232,240,248"
        && screen.pixel(10, 301).toString() === "248,252,248"
        && screen.pixel(120, 310).toString() === "0,132,208",
      { name: "browser-plugin-installed", timeoutMs: 20_000, intervalMs: 250 },
    );
    expect(browserPluginInstalled.uniqueColorCount()).toBeGreaterThan(2);

    // 确认插件安装结果并选择 CMNET，随后等待浏览器组件下载和首屏渲染。
    await engine.key("LEFT_SOFT", 3_000);
    await engine.key("LEFT_SOFT", 3_000);
    const browser = await engine.waitForScreen(
      screen => screen.uniqueColorCount() > 4
        && screen.pixel(120, 232).toString() === "248,252,248"
        && screen.pixel(58, 309).toString() === "80,148,216",
      { name: "browser-running", timeoutMs: 40_000, intervalMs: 250 },
    );
    expect(browser.diffPixelCount(adSelected, GAME_REGION)).toBeGreaterThan(1_000);

    // 浏览器右软键显示“返回”；确认退出后恢复到同一“火力全开”详情页。
    await engine.key("RIGHT_SOFT", 10_000);
    await engine.waitForScreen(
      screen => screen.diffPixelCount(adSelected, GAME_REGION) === 0,
      { name: "ad-after-browser-return", timeoutMs: 10_000, intervalMs: 100 },
    );
    await engine.key("RIGHT", 1_000);
    await engine.waitForScreen(
      screen => screen.pixel(0, 0).toString() === "104,184,224"
        && screen.diffPixelCount(fullPower, GAME_REGION) === 0,
      { name: "ad-after-layer-close", timeoutMs: 3_000, intervalMs: 100 },
    );
    await engine.key("DOWN", 1_000);
    await engine.waitForScreen(
      screen => screen.diffPixelCount(fullPower) === 0,
      { name: "full-power-returned", timeoutMs: 3_000, intervalMs: 100 },
    );

    await engine.key("RIGHT_SOFT", 1_000);
    await engine.waitForScreen(
      screen => screen.pixel(175, 103).toString() === "48,188,248",
      { name: "game-menu-returned", timeoutMs: 10_000, intervalMs: 100 },
    );
    await engine.key("RIGHT_SOFT", 1_000);
    const gameReturned = await engine.waitForScreen(
      screen => screen.pixel(22, 314).toString() === "200,252,248"
        && screen.diffPixelCount(gameMenu, GAME_REGION) > 1_000,
      { name: "game-returned", timeoutMs: 10_000, intervalMs: 100 },
    );
    expect(gameReturned.diffPixelCount(gameBeforeAd, GAME_REGION)).toBeGreaterThan(1_000);

    // 排除恢复边界的一次性快拍，再从每次 present 的 PPM 逐帧识别实际画面变化。
    await engine.delay(1_000);
    const resumed = await captureVisualRate(engine, "resumed");
    const visualRateRatio = resumed.visualRate / baseline.visualRate;
    const drawRateRatio = resumed.drawRate / baseline.drawRate;
    console.info(
      `[optwar-visual-rate] baseline=${baseline.visualRate.toFixed(3)} `
        + `resumed=${resumed.visualRate.toFixed(3)} ratio=${visualRateRatio.toFixed(3)} `
        + `draw-ratio=${drawRateRatio.toFixed(3)} changed=${resumed.changedPixels}`,
    );

    expect(visualRateRatio).toBeGreaterThanOrEqual(0.5);
    expect(visualRateRatio).toBeLessThanOrEqual(1.5);
    expect(drawRateRatio).toBeGreaterThanOrEqual(0.5);
    expect(drawRateRatio).toBeLessThanOrEqual(1.5);
    expect(resumed.last.diffPixelCount(resumed.first, GAME_REGION)).toBeGreaterThan(1_000);
    expect(resumed.last.pixel(22, 314)).toEqual([200, 252, 248]);
  });
});
