import { cpSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";


describe("gsha", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("downloads res", async () => {
    ws = await SkyEngineWorkspace.create();
    // The app expects its extracted resource index under mythroad/gsha.
    cpSync("test/fixtures/gsha", ws.path("mythroad/gsha"), { recursive: true });

    engine = await SkyEngineE2e.start("test/fixtures/gsha.mrp", {
      workDir: ws.dir,
      captureLatestFrame: true,
    });

    await vi.waitFor(async () => {
      const screen = await engine!.screen("bgm-select");
      // rgb(248, 252, 248)
      expect(screen.pixel(137, 162)).toEqual([248, 252, 248]);
    }, { timeout: 10_000, interval: 1_000 });

    // Keep music disabled so audio timing cannot change the network transition.
    await engine.key("RIGHT_SOFT", 1_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("menu");
      // rgb(0, 240, 248)
      expect(screen.pixel(94, 27)).toEqual([0, 240, 248]);
    }, { timeout: 10_000, interval: 1_000 });

    // 切换菜单 - 读取游戏
    await engine.key("RIGHT", 1_000);
    await engine.delay(1_000);
    // 进入读取游戏界面
    await engine.key("ENTER", 1_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("game-save");
      // rgb(0, 0, 0)
      expect(screen.pixel(120, 47)).toEqual([0, 0, 0]);
      // rgb(160, 180, 0)
      expect(screen.pixel(116, 13)).toEqual([160, 180, 0]);
    }, { timeout: 10_000, interval: 1_000 });
    
    // 读取存档
    await engine.key("ENTER", 1_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("game-save");
      // rgb(104, 104, 224)
      expect(screen.pixel(78, 183)).toEqual([104, 104, 224]);
    }, { timeout: 10_000, interval: 1_000 });

    // 确认注册
    await engine.key("LEFT_SOFT", 1_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("game-save");
      // rgb(248, 200, 136)
      expect(screen.pixel(134, 154)).toEqual([248, 200, 136]);
    }, { timeout: 10_000, interval: 1_000 });

    // 向右移动，触发资源下载
    await vi.waitFor(async () => {
      await engine!.key("RIGHT", 1_000);
      const screen = await engine!.screen("character-move");
      // rgb(40, 40, 40)
      expect(screen.pixel(29, 12)).toEqual([40, 40, 40]);
    }, { timeout: 10_000, interval: 1_000 });
    
    // 确定下载资源。500 ms guest timer 会在连接完成后轮询 socket 并发送请求。
    await engine.key("LEFT_SOFT", 1_000);
    const downloadStart = await engine.screen("download-start");
    let downloadProgress = downloadStart;
    await vi.waitFor(async () => {
      downloadProgress = await engine!.screen("download-progress");
      expect(downloadStart.diffPixelCount(downloadProgress)).toBeGreaterThan(0);
    }, { timeout: 30_000, interval: 500 });

    // 进程退出会刷新 C stdout；公网服务当前可能返回“资源不存在”，所以这里验证
    // 模拟器负责的边界：同一 socket 已连接，并完整发送正确的下载请求。
    await engine.stop();
    const stdout = await readFile(engine.stdoutPath, "utf8");
    // MSVC text stdout expands every LF to CRLF, including the request's existing
    // CRLF bytes, so captured HTTP lines become CRCRLF. Normalize host line endings
    // before applying the same protocol assertion on Windows and Unix.
    const normalizedStdout = stdout.replace(/\r+\n/g, "\n");
    const request = /my_getSocketState\((\d+)\): 0[\s\S]*?my_send\(s:\1, fd:\d+, len:(\d+)\): sent=(\d+),[^\n]*\n\[my_send\] data: POST \/simpleDownload HTTP\/1\.1\nHost: spd\.skymobiapp\.com:6009/.exec(normalizedStdout);
    expect(request, "resource download request was not sent on the connected socket").not.toBeNull();
    expect(request![3]).toBe(request![2]);

  });
});
