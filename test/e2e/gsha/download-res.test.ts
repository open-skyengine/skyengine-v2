import { cpSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

interface HttpCaptureServer {
  readonly port: number;
  readonly requests: Buffer[];
  close(): Promise<void>;
}

const NOT_FOUND_RESPONSE = Buffer.from(
  "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
  "ascii",
);

async function startHttpCaptureServer(): Promise<HttpCaptureServer> {
  const requests: Buffer[] = [];
  const sockets = new Set<Socket>();
  const server: Server = createServer(socket => {
    sockets.add(socket);
    let buffered = Buffer.alloc(0);
    socket.on("data", chunk => {
      buffered = Buffer.concat([buffered, chunk]);
      const headerEnd = buffered.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;

      const headers = buffered.subarray(0, headerEnd).toString("latin1");
      const contentLength = /^Content-Length:\s*(\d+)$/im.exec(headers);
      if (!contentLength) {
        socket.destroy();
        return;
      }
      const requestLength = headerEnd + 4 + Number(contentLength[1]);
      if (buffered.length < requestLength) return;

      requests.push(Buffer.from(buffered.subarray(0, requestLength)));
      socket.end(NOT_FOUND_RESPONSE);
    });
    socket.on("error", () => {});
    socket.on("close", () => sockets.delete(socket));
  });

  await new Promise<void>((resolve, reject) => {
    const onError = (error: Error) => reject(error);
    server.once("error", onError);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", onError);
      resolve();
    });
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    server.close();
    throw new Error("HTTP capture server did not expose a TCP port");
  }

  return {
    port: address.port,
    requests,
    async close() {
      for (const socket of sockets) socket.destroy();
      if (!server.listening) return;
      await new Promise<void>((resolve, reject) => {
        server.close(error => error ? reject(error) : resolve());
      });
    },
  };
}

function requestHead(request: Buffer): string {
  const headerEnd = request.indexOf("\r\n\r\n");
  return headerEnd < 0 ? "" : request.subarray(0, headerEnd).toString("latin1");
}

describe("gsha", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  let httpServer: HttpCaptureServer | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await httpServer?.close();
    httpServer = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("downloads res", async () => {
    ws = await SkyEngineWorkspace.create();
    // The app expects its extracted resource index under mythroad/gsha.
    cpSync("test/fixtures/gsha", ws.path("mythroad/gsha"), { recursive: true });
    httpServer = await startHttpCaptureServer();

    engine = await SkyEngineE2e.start("test/fixtures/gsha.mrp", {
      workDir: ws.dir,
      captureLatestFrame: true,
      // The app first connects to its fixed WAP proxy, then routes the resource
      // Host. Keep both protocol stages inside this test's loopback endpoint.
      dnsMap: [
        `10.0.0.172->127.0.0.1:${httpServer.port}`,
        `spd.skymobiapp.com->127.0.0.1:${httpServer.port}`,
      ].join(";"),
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
      await engine!.key("RIGHT", { holdMs: 250, waitForDraw: false });
      await engine!.delay(300);
      const screen = await engine!.screen("character-move");
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

    await vi.waitFor(() => {
      expect(
        httpServer!.requests.some(request => requestHead(request).startsWith("POST /simpleDownload HTTP/1.1")),
        "resource download request was not received by the loopback endpoint",
      ).toBe(true);
    }, { timeout: 10_000, interval: 100 });
    const request = httpServer.requests.find(request =>
      requestHead(request).startsWith("POST /simpleDownload HTTP/1.1")
    )!;
    const headerEnd = request.indexOf("\r\n\r\n");
    const headers = request.subarray(0, headerEnd).toString("latin1");
    expect(headers).toContain("\r\nHost: spd.skymobiapp.com:6009");
    const contentLength = Number(/^Content-Length:\s*(\d+)$/im.exec(headers)![1]);
    expect(contentLength).toBeGreaterThan(0);
    expect(request.length - headerEnd - 4).toBe(contentLength);
  });
});
