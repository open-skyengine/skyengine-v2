import { createHash } from "node:crypto";
import { readFileSync, rmSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type PpmImage, SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const TALKCAT_RESOURCE_APP_ID = 998_101;
const TALKCAT_RESOURCE_VERSION = 0;

interface TalkcatDownloadServer {
  readonly port: number;
  readonly requests: Buffer[];
  respond(): void;
  close(): Promise<void>;
}

function uint16(value: number): Buffer {
  const result = Buffer.alloc(2);
  result.writeUInt16BE(value);
  return result;
}

function uint32(value: number): Buffer {
  const result = Buffer.alloc(4);
  result.writeUInt32BE(value);
  return result;
}

function tlv(tag: number, value: Buffer): Buffer {
  return Buffer.concat([uint32(tag), uint32(value.length), value]);
}

function parseTlvFields(body: Buffer): Map<number, Buffer> {
  const fields = new Map<number, Buffer>();
  for (let offset = 0; offset < body.length;) {
    if (body.length - offset < 8) throw new Error("truncated simpleDownload TLV header");
    const tag = body.readUInt32BE(offset);
    const length = body.readUInt32BE(offset + 4);
    offset += 8;
    if (length > body.length - offset) throw new Error("truncated simpleDownload TLV value");
    fields.set(tag, body.subarray(offset, offset + length));
    offset += length;
  }
  return fields;
}

function requiredUInt32(fields: Map<number, Buffer>, tag: number): number {
  const value = fields.get(tag);
  if (value?.length !== 4) throw new Error(`simpleDownload field ${tag} is missing or malformed`);
  return value.readUInt32BE();
}

function crc32(bytes: Buffer): number {
  let crc = 0xffff_ffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb8_8320 : 0);
    }
  }
  return (crc ^ 0xffff_ffff) >>> 0;
}

function buildDownloadResponse(request: Buffer, resource: Buffer): Buffer {
  const headerEnd = request.indexOf("\r\n\r\n");
  if (headerEnd < 0) throw new Error("simpleDownload request has no HTTP header terminator");
  const fields = parseTlvFields(request.subarray(headerEnd + 4));
  const productId = requiredUInt32(fields, 0x2775);
  const appId = requiredUInt32(fields, 0x29ce);
  const resourceVersion = TALKCAT_RESOURCE_VERSION;
  if (resource.length < 0xc4 || resource.subarray(0, 4).toString("ascii") !== "MRPG") {
    throw new Error("talkcat download fixture is not a complete MRP package");
  }
  const resourcePackage = Buffer.from(resource);
  resourcePackage.fill(0, 0x10, 0x1c);
  resourcePackage.write(`${appId}.mrp`, 0x10, "ascii");
  resourcePackage.writeUInt32LE(appId, 0x44);
  resourcePackage.writeUInt32LE(resourceVersion, 0x48);
  resourcePackage.writeUInt32BE(appId, 0xc0);
  resourcePackage.writeUInt32BE(resourceVersion, 0xc4);
  resourcePackage.fill(0, 0x54, 0x58);
  resourcePackage.writeUInt32LE(crc32(resourcePackage), 0x54);
  const metadata = Buffer.concat([
    tlv(0x2c7, uint32(1)),
    tlv(0x2be, uint32(appId)),
    tlv(0x2bf, uint32(resourceVersion)),
    tlv(0x2c0, Buffer.alloc(0)),
    tlv(0x2c1, Buffer.alloc(0)),
    tlv(0x2c2, uint16(0)),
    tlv(0x2c3, uint32(0)),
    tlv(0x2c4, uint32(resourcePackage.length)),
    tlv(0x2c5, createHash("md5").update(resourcePackage).digest()),
    tlv(0x2c6, uint32(0)),
  ]);
  return Buffer.concat([
    tlv(0x64, uint32(200)),
    tlv(0x65, uint32(productId)),
    tlv(0x6d, Buffer.from("00000000000000000001", "ascii")),
    tlv(0x2bc, uint16(1)),
    tlv(0x2bd, metadata),
    tlv(0x2c3, uint32(0)),
    tlv(0x2d1, resourcePackage),
  ]);
}

async function startTalkcatDownloadServer(): Promise<TalkcatDownloadServer> {
  const resource = readFileSync("test/fixtures/gghjt/gghjtzy_res_mtk_12001.mrp");
  const requests: Buffer[] = [];
  const pendingResponses: Array<{ socket: Socket; request: Buffer }> = [];
  const sockets = new Set<Socket>();
  const server: Server = createServer(socket => {
    sockets.add(socket);
    let buffered = Buffer.alloc(0);
    let handled = false;
    socket.on("data", chunk => {
      if (handled) return;
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
      handled = true;
      const request = Buffer.from(buffered.subarray(0, requestLength));
      requests.push(request);
      pendingResponses.push({ socket, request });
    });
    socket.on("error", () => {});
    socket.on("close", () => sockets.delete(socket));
  });

  const respond = () => {
    const pending = pendingResponses.shift();
    if (!pending) throw new Error("talkcat download server has no pending request");
    const { socket, request } = pending;
    if (socket.destroyed) throw new Error("talkcat download request closed before its response");
    try {
      const body = buildDownloadResponse(request, resource);
      socket.end(Buffer.concat([
        Buffer.from(
          `HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: ${body.length}\r\nConnection: close\r\n\r\n`,
          "ascii",
        ),
        body,
      ]));
    } catch (error) {
      socket.destroy();
      throw error;
    }
  };

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
    throw new Error("talkcat download server did not expose a TCP port");
  }
  return {
    port: address.port,
    requests,
    respond,
    async close() {
      for (const socket of sockets) socket.destroy();
      if (!server.listening) return;
      await new Promise<void>((resolve, reject) => {
        server.close(error => error ? reject(error) : resolve());
      });
    },
  };
}

function downloadDnsMap(server: TalkcatDownloadServer): string {
  return [
    `10.0.0.172->127.0.0.1:${server.port}`,
    `spd.skymobiapp.com->127.0.0.1:${server.port}`,
  ].join(";");
}

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
  let downloadServer: TalkcatDownloadServer | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await downloadServer?.close();
    downloadServer = undefined;
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
  it("离线下载喝水资源包后保持运行", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/talkcat"), { force: true, recursive: true });
    downloadServer = await startTalkcatDownloadServer();

    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", {
      workDir: ws.dir,
      dnsMap: downloadDnsMap(downloadServer),
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
      await vi.waitFor(() => {
        expect(downloadServer!.requests.length).toBe(1);
      }, { timeout: 10_000, interval: 100 });
      const screen = await engine.screen("downloading");
      expect(downloadConfirm.diffPixelCount(screen)).toBeGreaterThan(0);
      expect(screen.pixel(78, 280)).not.toEqual([32, 64, 120]);
      const requestHead = downloadServer.requests[0]
        .subarray(0, downloadServer.requests[0].indexOf("\r\n\r\n"))
        .toString("latin1");
      expect(requestHead).toMatch(/^POST \/simpleDownload HTTP\/1\.1/m);
      expect(requestHead).toContain("\r\nHost: spd.skymobiapp.com:6009");
      const requestBodyOffset = downloadServer.requests[0].indexOf("\r\n\r\n") + 4;
      const requestFields = parseTlvFields(
        downloadServer.requests[0].subarray(requestBodyOffset),
      );
      expect(requiredUInt32(requestFields, 0x2775)).toBe(3_462);
      expect(requiredUInt32(requestFields, 0x29ce)).toBe(TALKCAT_RESOURCE_APP_ID);
      expect(requiredUInt32(requestFields, 0x29cf)).toBe(0);
      downloadServer.respond();
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
        timeout: 30_000,
        interval: 1_000
      })
    }
    {
      // 当前 profile 会消费下载缓存并显示失败提示；主操作可确定性进入重试进度。
      await engine.key("LEFT_SOFT", 1_000);
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("download-retry");
        expect(postDownload.diffPixelCount(screen)).toBeGreaterThan(0);
      }, {
        timeout: 30_000,
        interval: 1_000
      });

      await vi.waitFor(async () => {
        const screen = await engine!.screen("post-download-stable");
        expect(postDownload.diffPixelCount(screen)).toBe(0);
      }, { timeout: 10_000, interval: 100 });

      expect(await engine.waitForExit(100)).toBe(false);
      const stdout = readFileSync(engine.stdoutPath, "utf-8");
      expect(stdout).not.toContain("Invalid memory read");
    }
  });
  it("循环取消", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/talkcat"), { force: true, recursive: true });
    downloadServer = await startTalkcatDownloadServer();

    engine = await SkyEngineE2e.start("test/fixtures/talkcat.mrp", {
      workDir: ws.dir,
      dnsMap: downloadDnsMap(downloadServer),
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
