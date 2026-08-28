import { existsSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type PpmImage, SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const TALKCAT_DOWNLOAD_DNS_MAP = "spd.skymobiapp.com->159.75.119.124";

function countColor(
  image: PpmImage,
  color: readonly [number, number, number],
  rect: { x: number; y: number; width: number; height: number },
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      if (image.pixel(x, y).every((channel, index) => channel === color[index])) count++;
    }
  }
  return count;
}

describe("talkcat 进入游戏", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏启动正常", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/talkcat"), { force: true, recursive: true });

    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", { workDir: ws.dir });

    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("main");
      // rgb(232, 236, 232)
      expect(boot.pixel(27, 273)).toEqual([232, 236, 232]);
      // rgb(0, 12, 16)
      expect(boot.pixel(216, 27)).toEqual([0, 12, 16]);
      // rgb(64, 64, 64)
      expect(boot.pixel(221, 279)).toEqual([64, 64, 64]);
    }, { timeout: 90_000, interval: 1_000 });
  });
  it("关于帮助按钮可重复打开且保持运行", async () => {
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/talkcat"), { force: true, recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", { workDir: ws.dir });

    const main = await engine.waitForScreen(screen =>
      screen.pixel(27, 273).every((channel, index) => channel === [232, 236, 232][index])
      && screen.pixel(216, 27).every((channel, index) => channel === [0, 12, 16][index])
      && screen.pixel(221, 279).every((channel, index) => channel === [64, 64, 64][index]), {
        name: "about-help-main",
        timeoutMs: 90_000,
        intervalMs: 1_000,
      });

    const clickAndCapture = async (x: number, y: number, name: string): Promise<PpmImage> => {
      const accepted = await engine!.command(`CLICK ${x} ${y}`);
      const acceptedDraw = /^OK click draw_count (\d+)$/.exec(accepted);
      if (!acceptedDraw) throw new Error(`Unexpected CLICK response: ${accepted}`);
      const drawn = await engine!.command(`WAIT_DRAW ${acceptedDraw[1]} 30000`, 35_000);
      const helpDraw = /^OK draw_count (\d+)$/.exec(drawn);
      if (!helpDraw) throw new Error(`Unexpected WAIT_DRAW response: ${drawn}`);
      return engine!.screenDraw(Number(helpDraw[1]), name);
    };

    const expectHelpScreen = (help: PpmImage, previous: PpmImage): void => {
      expect(previous.diffPixelCount(help)).toBeGreaterThan(60_000);
      expect(help.pixel(50, 15)).toEqual([248, 252, 248]);
      expect(help.pixel(75, 15)).toEqual([248, 252, 248]);
      expect(
        countColor(help, [248, 252, 248], { x: 10, y: 5, width: 200, height: 305 }),
      ).toBeGreaterThan(2_000);
    };

    const help = await clickAndCapture(20, 66, "about-help");
    expectHelpScreen(help, main);

    await engine.delay(1_000);
    const returnClick = await engine.command("CLICK 20 66");
    expect(returnClick).toMatch(/^OK click draw_count \d+$/);
    const returned = await engine.waitForScreen(screen =>
      screen.pixel(27, 273).every((channel, index) => channel === [232, 236, 232][index])
      && screen.pixel(216, 27).every((channel, index) => channel === [0, 12, 16][index])
      && screen.pixel(221, 279).every((channel, index) => channel === [64, 64, 64][index]), {
        name: "about-help-returned",
        timeoutMs: 5_000,
        intervalMs: 100,
      });
    expect(help.diffPixelCount(returned)).toBeGreaterThan(60_000);
    expect(returned.pixel(27, 273)).toEqual([232, 236, 232]);
    expect(returned.pixel(216, 27)).toEqual([0, 12, 16]);
    expect(returned.pixel(221, 279)).toEqual([64, 64, 64]);

    await engine.delay(1_000);
    const reopenedHelp = await clickAndCapture(20, 66, "about-help-reopened");
    expectHelpScreen(reopenedHelp, returned);

    await engine.delay(1_000);
    const secondReturnClick = await engine.command("CLICK 20 66");
    expect(secondReturnClick).toMatch(/^OK click draw_count \d+$/);
    const returnedAgain = await engine.waitForScreen(screen =>
      screen.pixel(27, 273).every((channel, index) => channel === [232, 236, 232][index])
      && screen.pixel(216, 27).every((channel, index) => channel === [0, 12, 16][index])
      && screen.pixel(221, 279).every((channel, index) => channel === [64, 64, 64][index]), {
        name: "about-help-returned-second",
        timeoutMs: 5_000,
        intervalMs: 100,
      });
    expect(reopenedHelp.diffPixelCount(returnedAgain)).toBeGreaterThan(60_000);

    await engine.delay(1_000);
    const thirdHelp = await clickAndCapture(20, 66, "about-help-third");
    expectHelpScreen(thirdHelp, returnedAgain);
    expect(await engine.waitForExit(100)).toBe(false);
    const runtimeLog = readFileSync(engine.stderrPath, "utf-8");
    expect(runtimeLog).not.toMatch(/ARM fault|ABI error|panicked at|Invalid memory (?:read|write)/);
  });
  it("从159服务器下载喝水资源包后保持运行", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/talkcat"), { force: true, recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", {
      workDir: ws.dir,
      dnsMap: TALKCAT_DOWNLOAD_DNS_MAP,
    });

    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("main");
      // rgb(232, 236, 232)
      expect(boot.pixel(27, 273)).toEqual([232, 236, 232]);
      // rgb(0, 12, 16)
      expect(boot.pixel(216, 27)).toEqual([0, 12, 16]);
      // rgb(64, 64, 64)
      expect(boot.pixel(221, 279)).toEqual([64, 64, 64]);
    }, { timeout: 90_000, interval: 1_000 });

    let downloadConfirm: PpmImage;
    let postDownload: PpmImage;
    {
      // 点击水杯图标，触发下载提示
      await engine.click(22, 280, 1_000)
      await engine.delay(1_000)
      // 检查像素
      const screen = await engine.screen("download-confirm");
      downloadConfirm = screen;
      // rgb(32, 64, 120)
      expect(screen.pixel(78, 280)).toEqual([32, 64, 120]);
    }
    {
      // 点击确定开始下载
      await engine.click(78, 280, 1_000)
      const screen = await engine.screen("downloading");
      expect(downloadConfirm.diffPixelCount(screen)).toBeGreaterThan(0);
      expect(screen.pixel(78, 280)).not.toEqual([32, 64, 120]);
    }
    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("post-download");
        postDownload = screen;
        expect(
          countColor(screen, [32, 64, 120], { x: 50, y: 270, width: 140, height: 24 }),
        ).toBeGreaterThan(120);
      }, {
        timeout: 90_000,
        interval: 1_000
      })
    }
    {
      // 安装由服务端返回的 talkcat 资源包。
      await engine.key("LEFT_SOFT", 1_000);
      let installProgress: PpmImage | undefined;
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("download-retry");
        installProgress = screen;
        expect(postDownload.diffPixelCount(screen)).toBeGreaterThan(0);
      }, {
        timeout: 30_000,
        interval: 1_000
      });
      const capturedInstallProgress = installProgress;
      if (!capturedInstallProgress) {
        throw new Error("talkcat drink installation screen was not captured");
      }

      const drinkDir = ws.path("mythroad/talkcat/drink");
      let drinkAction: PpmImage | undefined;
      await vi.waitFor(async () => {
        expect(existsSync(`${drinkDir}/45`)).toBe(true);
        expect(existsSync(`${drinkDir}/sdrink2.mp3`)).toBe(true);
        const screen = await engine!.screen("drink-action");
        drinkAction = screen;
        expect(capturedInstallProgress.diffPixelCount(screen)).toBeGreaterThan(10_000);
      }, { timeout: 30_000, interval: 100 });
      if (!drinkAction) throw new Error("talkcat drink action screen was not captured");
      expect(readdirSync(drinkDir)).toHaveLength(47);
      for (let frame = 1; frame <= 45; frame++) {
        expect(statSync(`${drinkDir}/${frame}`).size).toBe(153_600);
      }

      await engine.delay(15_000);
      const stable = await engine.screen("post-download-stable");
      expect(drinkAction.diffPixelCount(stable)).toBeGreaterThan(10_000);

      expect(await engine.waitForExit(100)).toBe(false);
      const runtimeLog = readFileSync(engine.stderrPath, "utf-8");
      expect(runtimeLog).not.toMatch(/ARM fault|ABI error|panicked at|Invalid memory (?:read|write)/);
    }
  });
  it("从159服务器下载放屁资源包后保持运行", async () => {
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/talkcat"), { force: true, recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", {
      workDir: ws.dir,
      dnsMap: TALKCAT_DOWNLOAD_DNS_MAP,
    });

    await vi.waitFor(async () => {
      const boot = await engine!.screen("fart-main");
      expect(boot.pixel(27, 273)).toEqual([232, 236, 232]);
      expect(boot.pixel(216, 27)).toEqual([0, 12, 16]);
      expect(boot.pixel(221, 279)).toEqual([64, 64, 64]);
    }, { timeout: 90_000, interval: 1_000 });

    await engine.click(216, 194, 1_000);
    const downloadConfirm = await engine.screen("fart-download-confirm");
    expect(
      countColor(downloadConfirm, [32, 64, 120], { x: 50, y: 270, width: 140, height: 24 }),
    ).toBeGreaterThan(120);

    // 旧版 talkcat 的下载确认框由左软键提交，绘制出的按钮不稳定响应触控。
    await engine.key("LEFT_SOFT", { waitForDraw: false });
    let postDownload: PpmImage | undefined;
    await vi.waitFor(async () => {
      const screen = await engine!.screen("fart-post-download");
      postDownload = screen;
      expect(downloadConfirm.diffPixelCount(screen)).toBeGreaterThan(0);
      expect(
        countColor(screen, [32, 64, 120], { x: 50, y: 270, width: 140, height: 24 }),
      ).toBeGreaterThan(120);
    }, { timeout: 90_000, interval: 1_000 });
    if (!postDownload) throw new Error("talkcat fart download result was not captured");
    expect(statSync(ws.path("mythroad/talkcat/998113.mrp")).size).toBe(195_878);

    await engine.key("LEFT_SOFT", 1_000);
    const installProgress = await engine.screen("fart-installing");
    expect(postDownload.diffPixelCount(installProgress)).toBeGreaterThan(0);

    const fartDir = ws.path("mythroad/talkcat/fart");
    let fartAction: PpmImage | undefined;
    await vi.waitFor(async () => {
      expect(existsSync(`${fartDir}/17`)).toBe(true);
      expect(existsSync(`${fartDir}/sfart3.mp3`)).toBe(true);
      const screen = await engine!.screen("fart-action");
      fartAction = screen;
      expect(installProgress.diffPixelCount(screen)).toBeGreaterThan(10_000);
    }, { timeout: 30_000, interval: 100 });
    if (!fartAction) throw new Error("talkcat fart action screen was not captured");
    expect(readdirSync(fartDir)).toHaveLength(20);
    for (let frame = 1; frame <= 17; frame++) {
      expect(statSync(`${fartDir}/${frame}`).size).toBe(124_620);
    }
    expect(statSync(`${fartDir}/sfart1.mp3`).size).toBe(2_220);
    expect(statSync(`${fartDir}/sfart2.mp3`).size).toBe(3_265);
    expect(statSync(`${fartDir}/sfart3.mp3`).size).toBe(5_472);

    await engine.delay(15_000);
    const stable = await engine.screen("fart-stable");
    expect(fartAction.diffPixelCount(stable)).toBeGreaterThan(10_000);

    expect(await engine.waitForExit(100)).toBe(false);
    const runtimeLog = readFileSync(engine.stderrPath, "utf-8");
    expect(runtimeLog).not.toMatch(/ARM fault|ABI error|panicked at|Invalid memory (?:read|write)/);
  });
  it("循环取消", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/talkcat"), { force: true, recursive: true });

    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", {
      workDir: ws.dir,
      dnsMap: TALKCAT_DOWNLOAD_DNS_MAP,
    });

    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("main");
      // rgb(232, 236, 232)
      expect(boot.pixel(27, 273)).toEqual([232, 236, 232]);
      // rgb(0, 12, 16)
      expect(boot.pixel(216, 27)).toEqual([0, 12, 16]);
      // rgb(64, 64, 64)
      expect(boot.pixel(221, 279)).toEqual([64, 64, 64]);
    }, { timeout: 90_000, interval: 1_000 });
    for (let i = 0; i < 20; i++) {
      {
        // 点击水杯图标，触发下载提示
        await engine.click(139, 266, 1_000)
        await engine.delay(1_000)
        // 检查像素
        const screen = await engine.screen("download-confirm");
        // rgb(32, 64, 120)
        expect(screen.pixel(78, 280)).toEqual([32, 64, 120]);
      }
      {
        // 点击确定开始下载
        await engine.click(139, 266, 1_000)
        await engine.delay(1_000)
        // rgb(32, 212, 0)
        const screen = await engine.screen("download-cancel");
        // rgb(32, 64, 120)
        expect(screen.pixel(78, 280)).not.toEqual([32, 64, 120]);
        await engine.delay(1_000)
      }
    }
  });
});
