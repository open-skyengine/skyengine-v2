import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage } from "../engine-e2e.js";
import { cpSync } from "node:fs";
import { readFile } from "node:fs/promises";

const NETPAY_APPID = 480010;
const NETPAY_UPDATE_FIXTURE = "test/fixtures/plugins/netpay-original.mrp";

interface CapturedDrawFrame {
  readonly draw: number;
  readonly image: PpmImage;
}

async function expectNetpayFixtureInstalled(runtimePath: string): Promise<void> {
  const [installed, expected] = await Promise.all([
    readFile(runtimePath),
    readFile(NETPAY_UPDATE_FIXTURE),
  ]);
  expect(installed.equals(expected), "runtime netpay.mrp was not replaced by the update fixture").toBe(true);
  expect(readMrpIdentity(installed)).toEqual({ appId: NETPAY_APPID, version: 370 });
}

async function isNetpayFixtureInstalled(runtimePath: string): Promise<boolean> {
  const [installed, expected] = await Promise.all([
    readFile(runtimePath),
    readFile(NETPAY_UPDATE_FIXTURE),
  ]);
  return installed.equals(expected);
}

function readMrpIdentity(mrp: Buffer): { appId: number; version: number } {
  expect(mrp.subarray(0, 4).toString("ascii")).toBe("MRPG");
  return {
    appId: mrp.readUInt32LE(68),
    version: mrp.readUInt32LE(72),
  };
}

async function expectSimpleDownloadSent(stdoutPath: string): Promise<void> {
  const stdout = await readFile(stdoutPath, "utf8");
  expect(
    stdout,
    "simpleDownload did not connect directly to the configured 159 server",
  ).toMatch(/dns map: spd\.skymobiapp\.com -> 159\.75\.119\.124[\s\S]*?my_connect\(fd:\d+, '159\.75\.119\.124', 6009\)[\s\S]*?my_connect\(0x[0-9A-F]+\) suc/);
  const request = /my_getSocketState\((\d+)\): 0[\s\S]*?my_send\(s:\1, fd:\d+, len:(\d+)\): sent=(\d+),[^\n]*\n\[my_send\] data: POST \/simpleDownload HTTP\/1\.1\r?\nHost: spd\.skymobiapp\.com:6009/.exec(stdout);
  expect(request, "netpay plugin download request was not sent on the connected socket").not.toBeNull();
  expect(request![3]).toBe(request![2]);
}

function pixelEquals(image: PpmImage, x: number, y: number, expected: readonly [number, number, number]): boolean {
  const actual = image.pixel(x, y);
  return actual[0] === expected[0] && actual[1] === expected[1] && actual[2] === expected[2];
}

function isNetpayPanel(image: PpmImage): boolean {
  return pixelEquals(image, 5, 5, [0, 96, 200])
    && pixelEquals(image, 100, 30, [104, 104, 224])
    && pixelEquals(image, 5, 300, [248, 252, 248]);
}

async function captureNextDrawBatch(
  engine: SkyEngineE2e,
  drawCount: number,
  namePrefix: string,
  timeoutMs = 250,
): Promise<{ drawCount: number; frames: CapturedDrawFrame[] }> {
  const response = await engine.command(`WAIT_DRAW ${drawCount} ${timeoutMs}`, timeoutMs + 500);
  const match = /^OK draw_count (\d+)$/.exec(response);
  if (!match) throw new Error(`Unexpected WAIT_DRAW response: ${response}`);

  const nextDrawCount = Number(match[1]);
  const frames: CapturedDrawFrame[] = [];
  for (let draw = drawCount + 1; draw <= nextDrawCount; draw += 1) {
    const image = await engine.screenDraw(draw, `${namePrefix}-${String(draw).padStart(4, "0")}`);
    frames.push({ draw, image });
    console.info(`[gjxwsmn] saved draw=${draw} colors=${image.uniqueColorCount()}`);
  }
  return { drawCount: nextDrawCount, frames };
}

function isWaitDrawTimeout(error: unknown): boolean {
  return error instanceof Error && error.message.includes("wait_draw_timeout");
}

async function captureUntilUpdatePrompt(
  engine: SkyEngineE2e,
  startDrawCount: number,
): Promise<{ drawCount: number; payScreen: PpmImage; frames: CapturedDrawFrame[] }> {
  const deadline = Date.now() + 15_000;
  const frames: CapturedDrawFrame[] = [];
  let drawCount = startDrawCount;
  let payScreen: PpmImage | undefined;

  while (Date.now() < deadline) {
    try {
      const batch = await captureNextDrawBatch(engine, drawCount, "post-pay");
      drawCount = batch.drawCount;
      frames.push(...batch.frames);
      for (const frame of batch.frames) {
        if (!payScreen && isNetpayPanel(frame.image)) {
          payScreen = frame.image;
          continue;
        }
        if (payScreen && isNetpayPanel(frame.image)
          && payScreen.diffPixelCount(frame.image, { x: 0, y: 28, width: 240, height: 96 }) > 500) {
          return { drawCount, payScreen, frames };
        }
      }
    } catch (error) {
      if (!isWaitDrawTimeout(error)) throw error;
    }
  }
  throw new Error("netpay update confirmation was not drawn after the pay response");
}

async function captureUntilInstalled(
  engine: SkyEngineE2e,
  startDrawCount: number,
  runtimePath: string,
): Promise<CapturedDrawFrame[]> {
  const deadline = Date.now() + 30_000;
  const frames: CapturedDrawFrame[] = [];
  let drawCount = startDrawCount;
  let quietAfterInstall = 0;

  while (Date.now() < deadline) {
    try {
      const batch = await captureNextDrawBatch(engine, drawCount, "plugin-update");
      drawCount = batch.drawCount;
      frames.push(...batch.frames);
      quietAfterInstall = 0;
    } catch (error) {
      if (!isWaitDrawTimeout(error)) throw error;
      if (await isNetpayFixtureInstalled(runtimePath)) {
        quietAfterInstall += 1;
        if (quietAfterInstall >= 2) return frames;
      }
    }
  }
  throw new Error("netpay plugin update did not finish within 30 seconds");
}

function progressFillWidth(image: PpmImage): number | undefined {
  const y = 160;
  if (!pixelEquals(image, 66, y, [248, 252, 248])
    || !pixelEquals(image, 67, y, [0, 0, 0])
    || !pixelEquals(image, 172, y, [0, 0, 0])
    || !pixelEquals(image, 173, y, [248, 252, 248])) {
    return undefined;
  }

  let width = 0;
  while (width < 104 && pixelEquals(image, 68 + width, y, [0, 200, 200])) {
    width += 1;
  }
  for (let x = 68 + width; x <= 171; x += 1) {
    if (!pixelEquals(image, x, y, [0, 0, 0])) return undefined;
  }
  return width;
}

function expectRealDownloadProgress(frames: CapturedDrawFrame[]): void {
  const widths = frames
    .map(frame => progressFillWidth(frame.image))
    .filter((width): width is number => width !== undefined);

  expect(widths.length, "netpay did not draw its download progress bar").toBeGreaterThan(2);
  expect(widths[0], "download progress did not start from an empty bar").toBe(0);
  expect(widths.at(-1), "download progress did not reach a full bar").toBe(104);
  expect(new Set(widths).size, "download progress did not expose multiple real values").toBeGreaterThan(2);
  for (let index = 1; index < widths.length; index += 1) {
    expect(widths[index], "download progress moved backwards").toBeGreaterThanOrEqual(widths[index - 1]);
  }
}

async function waitForBgmPrompt(engine: SkyEngineE2e, name: string): Promise<void> {
  await vi.waitFor(async () => {
    const screen = await engine.screen(name);
    // rgb(232, 48, 0)
    expect(screen.pixel(147, 87)).toEqual([232, 48, 0]);
    // rgb(176, 192, 208)
    expect(screen.pixel(116, 122)).toEqual([176, 192, 208]);
  }, { timeout: 10_000, interval: 1_000 });
}

async function waitForMenu(engine: SkyEngineE2e, name: string): Promise<void> {
  await vi.waitFor(async () => {
    const screen = await engine.screen(name);
    // rgb(24, 8, 16)
    expect(screen.pixel(206, 44)).toEqual([24, 8, 16]);
    // rgb(248, 192, 192)
    expect(screen.pixel(74, 219)).toEqual([248, 192, 192]);
  }, { timeout: 10_000, interval: 1_000 });
}


describe("gjxwsmn", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("付费", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    cpSync("test/fixtures/gjxwsmn", ws.path("mythroad/gjxwsmn"), { recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/gjxwsmn.mrp", {
      workDir: ws.dir,
      dnsMap: 'rop.skymobiapp.com->127.0.0.1:8088;spd.skymobiapp.com->159.75.119.124',
      // Match the fixture's persisted device date to isolate payment state
      // from the game's unrelated daily-state rollover.
      deviceDate: "2026-07-23",
    });

    {
      if (!engine) throw new Error("engine is undefined");
      await waitForBgmPrompt(engine, `bgm-select-1`);

      // 取消背景音乐，进入菜单
      await engine.key('RIGHT_SOFT', 1_000);
      await waitForMenu(engine, `menu-1`);

      // 进入默认菜单，存档界面
      await engine.key('ENTER', 1_000);
      await vi.waitFor(async () => {
        const screen = await engine!.screen('save-1.0');
        // rgb(248, 220, 144)
        expect(screen.pixel(173, 156)).toEqual([248, 220, 144]);
        // rgb(48, 52, 16)
        expect(screen.pixel(41, 39)).toEqual([48, 52, 16]);
      }, { timeout: 10_000, interval: 1_000 });

      // 回车进入第一个存档
      const prePayDraw = await engine.drawCount();
      await engine.key('ENTER', 1_000);

      // 从 pay 请求开始逐帧保存。更新提示出现后才确认下载，避免静默更新
      // 绕过 netpay 自己的真实进度界面。
      const confirmation = await captureUntilUpdatePrompt(engine, prePayDraw);
      await engine.key("LEFT_SOFT", { waitForDraw: false });

      const runtimeNetpay = ws.path("mythroad/plugins/netpay.mrp");
      const updateFrames = await captureUntilInstalled(
        engine,
        confirmation.drawCount,
        runtimeNetpay,
      );
      expectRealDownloadProgress(updateFrames);
      await expectNetpayFixtureInstalled(runtimeNetpay);

      const stdoutPath = engine.stdoutPath;
      await expectSimpleDownloadSent(stdoutPath);
    }
    {
      await vi.waitFor(async () => {
        const screen = await engine!.screen('save-1.1');
        // rgb(248, 220, 144)
        expect(screen.pixel(173, 156)).toEqual([248, 220, 144]);
        // rgb(48, 52, 16)
        expect(screen.pixel(41, 39)).toEqual([48, 52, 16]);
      }, { timeout: 10_000, interval: 1_000 });
      await engine.key('ENTER', 1_000);
      await engine.delay(3_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('scene-1.0');
        // rgb(136, 212, 64)
        expect(screen.pixel(29, 40)).toEqual([136, 212, 64]);
        // rgb(136, 212, 64)
        expect(screen.pixel(221, 219)).toEqual([136, 212, 64]);
      }, { timeout: 10_000, interval: 1_000 });
      // 打开商品
      await engine.key('RIGHT_SOFT', 1_000);
      await vi.waitFor(async () => {
        const screen = await engine!.screen('store-1.0');
        // rgb(232, 164, 72)
        expect(screen.pixel(45, 62)).toEqual([232, 164, 72]);
      }, { timeout: 10_000, interval: 1_000 });
      await engine.key('ENTER', 1_000);
      await vi.waitFor(async () => {
        const screen = await engine!.screen('pay-1.0');
        // rgb(104, 104, 224)
        expect(screen.pixel(83, 150)).toEqual([104, 104, 224]);
      }, { timeout: 10_000, interval: 1_000 });
      await engine.delay(17_000)
      await vi.waitFor(async () => {
        const screen = await engine!.screen('pay-end-1.0');
        // PROP 成功后回到商城，第一项由“未开”更新为“开通”。这两个文字像素
        // 同时区别于付款前商城和蓝色的网络错误页。
        expect(screen.pixel(133, 55)).toEqual([232, 164, 72]);
        expect(screen.pixel(137, 55)).toEqual([104, 36, 0]);
      }, { timeout: 10_000, interval: 1_000 });

      // netpay 仅在服务端完成动作被接受后写入应用级授权文件。
      const entitlement = await readFile(ws!.path("mythroad/gjxwsmn/combo.sid"));
      expect(entitlement.length).toBeGreaterThan(0);
    }
  });
});
