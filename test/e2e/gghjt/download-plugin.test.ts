import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs, { cpSync } from "fs";
import { createHash } from "node:crypto";
import { createServer, type Server, type Socket } from "node:net";

interface NetpayDownloadServer {
  readonly port: number;
  readonly requests: Buffer[];
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

function buildSimpleDownloadResponse(request: Buffer, plugin: Buffer): Buffer {
  const headerEnd = request.indexOf("\r\n\r\n");
  if (headerEnd < 0) throw new Error("simpleDownload request has no HTTP header terminator");
  const fields = parseTlvFields(request.subarray(headerEnd + 4));
  const productId = requiredUInt32(fields, 0x2775);
  const appId = requiredUInt32(fields, 0x29ce);
  const metadata = Buffer.concat([
    tlv(0x2c7, uint32(1)),
    tlv(0x2be, uint32(appId)),
    tlv(0x2bf, uint32(0)),
    tlv(0x2c0, Buffer.alloc(0)),
    tlv(0x2c1, Buffer.alloc(0)),
    tlv(0x2c2, uint16(0)),
    tlv(0x2c3, uint32(0)),
    tlv(0x2c4, uint32(plugin.length)),
    tlv(0x2c5, createHash("md5").update(plugin).digest()),
    tlv(0x2c6, uint32(0)),
  ]);
  return Buffer.concat([
    tlv(0x64, uint32(200)),
    tlv(0x65, uint32(productId)),
    tlv(0x6d, Buffer.from("00000000000000000001", "ascii")),
    tlv(0x2bc, uint16(1)),
    tlv(0x2bd, metadata),
    tlv(0x2c3, uint32(0)),
    tlv(0x2d1, plugin),
  ]);
}

async function startNetpayDownloadServer(): Promise<NetpayDownloadServer> {
  const plugin = fs.readFileSync("test/fixtures/plugins/netpay.mrp");
  const requests: Buffer[] = [];
  const sockets = new Set<Socket>();
  const server: Server = createServer(socket => {
    sockets.add(socket);
    let buffered = Buffer.alloc(0);
    let responded = false;
    socket.on("data", chunk => {
      if (responded) return;
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
      responded = true;
      const request = Buffer.from(buffered.subarray(0, requestLength));
      requests.push(request);
      try {
        const responseBody = buildSimpleDownloadResponse(request, plugin);
        const responseHead = Buffer.from(
          `HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: ${responseBody.length}\r\nConnection: close\r\n\r\n`,
          "ascii",
        );
        socket.end(Buffer.concat([responseHead, responseBody]));
      } catch {
        socket.destroy();
      }
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
    throw new Error("netpay download server did not expose a TCP port");
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

function downloadDnsMap(server: NetpayDownloadServer, extra: string[] = []): string {
  return [
    `10.0.0.172->127.0.0.1:${server.port}`,
    `spd.skymobiapp.com->127.0.0.1:${server.port}`,
    ...extra,
  ].join(";");
}

describe("gghjt pixel flow", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  let downloadServer: NetpayDownloadServer | undefined;
  const memCheckTime = 10_000

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await downloadServer?.close();
    downloadServer = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("下载付费插件 - 直接返回", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    fs.rmSync(ws.path('mythroad/plugins/netpay.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/gghjt'), { force: true, recursive: true });
    fs.rmSync(ws.path('mythroad/cache'), { force: true, recursive: true });
    fs.cpSync('test/fixtures/gghjt', ws.path('mythroad/gghjt'), { recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", { workDir: ws.dir });

    {
      // 检测内存
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(1_000);
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const boot = await engine.screen("bgm-select");
        // rgb(0, 0, 0)
        expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);
        // rgb(248, 252, 248)
        expect(boot.pixel(84, 79)).toEqual([248, 252, 248]);
      }, {
        timeout: 30_000,
        interval: 1_000
      })
    }

    // 是否开启音乐？-> 否
    await engine.click(230, 308, 1_000);
    await engine.delay(1_000);

    // 跳过启动剧情
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        // 进入主菜单
        const screen = await engine.screen("menu");
        // rgb(152, 112, 32)
        expect(screen.pixel(110, 27)).toEqual([152, 112, 32]);
      }, {
        timeout: 30_000,
        interval: 1_000
      })
    }
    {
        // 切换菜单
        await engine.click(162, 291, 3_000);
        await engine.delay(1_000);
        await vi.waitFor(async () => {
          if (!engine) throw new Error("engine is undefined");
          const screen = await engine.screen("continueMenu");
          // rgb(232, 196, 104)
          expect(screen.pixel(162, 291)).toEqual([232, 196, 104 ]);
        }, {
          timeout: 30_000,
          interval: 1_000
        })
    }
    // 点击继续游戏，进入插件下载界面
    await engine.click(116, 291, 3_000);
    await engine.delay(1_000);
    const menu = await engine.screen("download-plugin");
    // rgb(0, 4, 0)
    expect(menu.pixel(80, 80)).toEqual([0, 4, 0]);
  });
  it("下载付费插件 - 返回重进", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    fs.rmSync(ws.path('mythroad/plugins/netpay.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/gghjt'), { force: true, recursive: true });
    fs.rmSync(ws.path('mythroad/cache'), { force: true, recursive: true });
    fs.cpSync('test/fixtures/gghjt', ws.path('mythroad/gghjt'), { recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", { workDir: ws.dir });

    {
      // 检测内存
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(1_000);

    }
    await engine.delay(memCheckTime);
    const boot = await engine.screen("bgm-select");
    // rgb(72,88,0)
    expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);

    // 是否开启音乐？-> 否
    await engine.click(230, 308, 1_000);
    await engine.delay(1_000);

    // 跳过启动剧情
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(152, 112, 32)
    expect(wake.pixel(110, 27)).toEqual([152, 112, 32]);

    // 切换菜单
    await engine.click(162, 291, 3_000);
    await engine.delay(1_000);
    // rgb(232, 196, 104)
    expect(wake.pixel(162, 291)).toEqual([232, 196, 104 ]);

    // 点击继续游戏，进入插件下载界面
    await engine.click(116, 291, 3_000);
    await engine.delay(1_000);
    const menu1 = await engine.screen("download-plugin");
    // rgb(0, 4, 0)
    expect(menu1.pixel(80, 80)).toEqual([0, 4, 0]);
    
    // 取消下载返回主菜单
    await engine.click(227, 308, 1_000);
    await engine.delay(2_000);
    // 切换菜单
    await engine.click(162, 291, 3_000);
    await engine.delay(1_000);
    // rgb(232, 196, 104)
    expect(wake.pixel(162, 291)).toEqual([232, 196, 104 ]);

    
    // 点击继续游戏，第二次进入插件下载界面
    await engine.click(116, 291, 3_000);
    await engine.delay(1_000);
    const menu2 = await engine.screen("download-plugin");
    // rgb(0, 4, 0)
    expect(menu2.pixel(80, 80)).toEqual([0, 4, 0]);
  
  });
  it("下载付费插件 - 下载完毕", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    fs.rmSync(ws.path('mythroad/plugins/netpay.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/gghjt'), { force: true, recursive: true });
    fs.rmSync(ws.path('mythroad/cache'), { force: true, recursive: true });
    fs.cpSync('test/fixtures/gghjt', ws.path('mythroad/gghjt'), { recursive: true });
    downloadServer = await startNetpayDownloadServer();
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", {
      workDir: ws.dir,
      dnsMap: downloadDnsMap(downloadServer),
    });

    {
      // 检测内存
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(1_000);

    }
    await engine.delay(memCheckTime);
    const boot = await engine.screen("bgm-select");
    // rgb(72,88,0)
    expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);

    // 是否开启音乐？-> 否
    await engine.click(230, 308, 1_000);
    await engine.delay(1_000);

    // 跳过启动剧情
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(152, 112, 32)
    expect(wake.pixel(110, 27)).toEqual([152, 112, 32]);

    // 切换菜单
    await engine.click(162, 291, 3_000);
    await engine.delay(1_000);
    const continueMenu = await engine.screen("continueMenu");
    // rgb(232, 196, 104)
    expect(continueMenu.pixel(162, 291)).toEqual([232, 196, 104 ]);

    // 点击继续游戏，进入插件下载界面
    await engine.click(116, 291, 3_000);
    await engine.delay(1_000);
    const menu1 = await engine.screen("download-plugin");
    // rgb(0, 4, 0)
    expect(menu1.pixel(80, 80)).toEqual([0, 4, 0]);
    
    // 确定下载
    await engine.click(5, 308, 1_000);
    // 检查下载进度和完成状态。
    await engine.waitForPixel(70, 176, [0, 252, 0], {
      name: "download-ing",
      timeoutMs: 20_000,
      intervalMs: 250,
    });
    await engine.waitForPixel(101, 148, [0, 252, 0], {
      name: "download-end",
      timeoutMs: 20_000,
      intervalMs: 250,
    });
    expect(downloadServer!.requests.length).toBeGreaterThan(0);
  
    // 点击确定进入付费界面
    await engine.click(15, 308, 1_000);
    await engine.delay(2_000);
    const pay = await engine.screen("pay-start");
    expect(pay.pixel(104, 147)).toEqual([104, 104, 224]);
  });
  it("下载付费插件 - 下载完毕返回重进", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    fs.rmSync(ws.path('mythroad/plugins/netpay.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/gghjt'), { force: true, recursive: true });
    fs.rmSync(ws.path('mythroad/cache'), { force: true, recursive: true });
    fs.cpSync('test/fixtures/gghjt', ws.path('mythroad/gghjt'), { recursive: true });
    downloadServer = await startNetpayDownloadServer();
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", {
      workDir: ws.dir,
      dnsMap: downloadDnsMap(downloadServer),
    });

    {
      // 检测内存
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(1_000);

    }
    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("bgm-select");
      // rgb(0,0,0)
      expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);
      // rgb(248, 252, 248)
      expect(boot.pixel(131, 77)).toEqual([248, 252, 248]);
    }, { timeout: 10_000, interval: 1_000 });

    // 是否开启音乐？-> 否
    await engine.click(230, 308, 1_000);
    await engine.delay(1_000);

    // 跳过启动剧情
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(152, 112, 32)
    expect(wake.pixel(110, 27)).toEqual([152, 112, 32]);

    // 切换菜单
    await engine.click(162, 291, 3_000);
    await engine.delay(1_000);
    const continueMenu = await engine.screen("continueMenu");
    // rgb(232, 196, 104)
    expect(continueMenu.pixel(162, 291)).toEqual([232, 196, 104 ]);

    // 点击继续游戏，进入插件下载界面
    await engine.click(116, 291, 3_000);
    await engine.delay(1_000);
    const menu1 = await engine.screen("download-plugin");
    // rgb(0, 4, 0)
    expect(menu1.pixel(80, 80)).toEqual([0, 4, 0]);
    
    // 确定下载
    await engine.click(5, 308, 1_000);
    await engine.waitForPixel(70, 176, [0, 252, 0], {
      name: "download-ing",
      timeoutMs: 20_000,
      intervalMs: 250,
    });
    await engine.waitForPixel(101, 148, [0, 252, 0], {
      name: "download-end",
      timeoutMs: 20_000,
      intervalMs: 250,
    });
    expect(downloadServer!.requests.length).toBeGreaterThan(0);

    // 替换netpay.mrp
    cpSync('test/fixtures/plugins/netpay.mrp', ws.path('mythroad/plugins/netpay.mrp'), { force: true });
  
    // 点击确定进入付费界面
    await engine.key('LEFT_SOFT', 1_000);
    
    await engine.delay(2_000);
    const pay = await engine.screen("pay-start");
    expect(pay.pixel(104, 147)).toEqual([104, 104, 224]);
    expect(pay.pixel(12, 302)).toEqual([248, 252, 248]);

    {
      // 取消下载返回主菜单
      await engine.click(227, 308, 1_000);
      await engine.delay(2_000);
      const screen = await engine.screen("menu-default");
      expect(screen.pixel(162, 291)).toEqual([232, 196, 104 ]);
    }
    {
      // 切换菜单
      await engine.click(162, 291, 3_000);
      await engine.delay(1_000);
      const screen = await engine.screen("menu-continue");
      // rgb(232, 196, 104)
      expect(screen.pixel(162, 291)).toEqual([232, 196, 104 ]);
    }
    {
      // 点击继续游戏，进入付费界面（前面下载完付费插件了）
      await engine.click(116, 291, 3_000);
      await engine.delay(1_000);
      const screen = await engine.screen("pay-start-2");
      // rgb(104, 104, 224)
      expect(screen.pixel(104, 147)).toEqual([104, 104, 224]);
    }
  });
  it("下载付费插件 - 下载完毕付费超时返回重进", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    fs.rmSync(ws.path('mythroad/plugins/netpay.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/gghjt'), { force: true, recursive: true });
    fs.rmSync(ws.path('mythroad/cache'), { force: true, recursive: true });
    fs.cpSync('test/fixtures/gghjt', ws.path('mythroad/gghjt'), { recursive: true });
    downloadServer = await startNetpayDownloadServer();
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", {
      workDir: ws.dir,
      dnsMap: downloadDnsMap(downloadServer, ["rop.skymobiapp.com->127.0.0.1"]),
    });

    {
      // 检测内存
      await engine.delay(1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.delay(1_000);

    }
    await vi.waitFor(async () => {
      if (!engine) throw new Error("engine is undefined");
      const boot = await engine.screen("bgm-select");
      // rgb(0,0,0)
      expect(boot.pixel(227, 308)).toEqual([0, 0, 0]);
      // rgb(248, 252, 248)
      expect(boot.pixel(131, 77)).toEqual([248, 252, 248]);
    }, { timeout: 10_000, interval: 1_000 });

    // 是否开启音乐？-> 否
    await engine.click(230, 308, 1_000);
    await engine.delay(1_000);

    // 跳过启动剧情
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);
    await engine.click(227, 308, 1_000);
    await engine.delay(1_000);

    // 进入主菜单
    const wake = await engine.screen("menu");
    // rgb(152, 112, 32)
    expect(wake.pixel(110, 27)).toEqual([152, 112, 32]);

    // 切换菜单
    await engine.click(162, 291, 3_000);
    await engine.delay(1_000);
    const continueMenu = await engine.screen("continueMenu");
    // rgb(232, 196, 104)
    expect(continueMenu.pixel(162, 291)).toEqual([232, 196, 104 ]);

    // 点击继续游戏，进入插件下载界面
    await engine.click(116, 291, 3_000);
    await engine.delay(1_000);
    const menu1 = await engine.screen("download-plugin");
    // rgb(0, 4, 0)
    expect(menu1.pixel(80, 80)).toEqual([0, 4, 0]);
    
    // 确定下载
    await engine.click(5, 308, 1_000);
    await engine.waitForPixel(70, 176, [0, 252, 0], {
      name: "download-ing",
      timeoutMs: 20_000,
      intervalMs: 250,
    });
    await engine.waitForPixel(101, 148, [0, 252, 0], {
      name: "download-end",
      timeoutMs: 20_000,
      intervalMs: 250,
    });
    expect(downloadServer!.requests.length).toBeGreaterThan(0);
  
    // 点击确定进入付费界面
    await engine.click(15, 308, 1_000);

    // 替换netpay.mrp
    cpSync('test/fixtures/plugins/netpay.mrp', ws.path('mythroad/plugins/netpay.mrp'), { force: true });
    await engine.delay(2_000);
    const pay = await engine.screen("pay-start");
    expect(pay.pixel(104, 147)).toEqual([104, 104, 224]);

    {
      // 等待付费超时
      await vi.waitFor(async () => {
        const pay = await engine!.screen("pay-timeout");
        // 确定按钮消失
        // rgb(0, 104, 208)
        expect(pay.pixel(12, 302)).toEqual([0, 104, 208]);
      }, {
        timeout: 60_000,
        interval: 1_000
      })
    }
    {
      // 取消下载返回主菜单
      await engine.click(227, 308, 1_000);
      await engine.delay(2_000);
      const screen = await engine.screen("menu-default");
      expect(screen.pixel(162, 291)).toEqual([232, 196, 104 ]);
    }
    {
      // 切换菜单
      await engine.click(162, 291, 3_000);
      await engine.delay(1_000);
      const screen = await engine.screen("menu-continue");
      // rgb(232, 196, 104)
      expect(screen.pixel(162, 291)).toEqual([232, 196, 104 ]);
    }
    {
      // 点击继续游戏，进入付费界面（前面下载完付费插件了）
      await engine.click(116, 291, 3_000);
      await engine.delay(1_000);
      const screen = await engine.screen("pay-start-2");
      // rgb(104, 104, 224)
      expect(screen.pixel(104, 147)).toEqual([104, 104, 224]);
    }
  });
});
