import { createHash } from "node:crypto";
import { copyFile, readFile, rm, stat } from "node:fs/promises";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage } from "../engine-e2e.js";

// Pinned op6120 package, SHA-256:
// 611b4cd737dcf458370ff215bc73636cf65bdc6dfc36907c2ce8aa8f00b7c8e2.
const MRP_SHA256 = "611b4cd737dcf458370ff215bc73636cf65bdc6dfc36907c2ce8aa8f00b7c8e2";
const SKY_SHA256 = "6db8e39e624a3606ce40e4cc5cc4c42f06a30055f625c4a28e6f51df66f56def";
const STARTED_FRAME_SHA256 = "c84009d93e752378bfaee8d98ab972ae5c2474d447b4e2c3895db7a7228b60d8";
const LOCAL_DNS_MAP = [
  "wap.skmeg.com->127.0.0.1",
  "rop.skymobiapp.com->127.0.0.1",
  "spd.skymobiapp.com->127.0.0.1",
  "freeads.51mrp.com->127.0.0.1",
  "proxy.51mrp.com->127.0.0.1",
  "proxy2.51mrp.com->127.0.0.1",
  "help.proxy.51mrp.com->127.0.0.1",
].join(";");

async function sha256(file: string): Promise<string> {
  return createHash("sha256").update(await readFile(file)).digest("hex");
}

async function waitForStartedFrame(engine: SkyEngineE2e): Promise<PpmImage> {
  let frame: PpmImage | undefined;
  await vi.waitFor(async () => {
    frame = await engine.screen("started");
    expect(await sha256(path.join(engine.tmpDir, "started.ppm"))).toBe(STARTED_FRAME_SHA256);
  }, { timeout: 90_000, interval: 500 });
  if (!frame) throw new Error("op6120 started frame was not captured");
  return frame;
}

describe("op6120 cold boot", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("allocates Flash SCRRAM outside LG_mem and completes startup", async () => {
    expect(await sha256("test/fixtures/op6120.mrp")).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    const mrp = ws.path("mythroad/op6120.mrp");
    const sky = ws.path("mythroad/cache/op6120/sky.bmp");
    await copyFile("test/fixtures/op6120.mrp", mrp);
    await copyFile("test/fixtures/plugins/advbar.mrp", ws.path("mythroad/plugins/advbar.mrp"));
    // Force the resource-conversion path whose 10 MiB SCRRAM request exposed
    // the allocator boundary; a warm cache would skip the original failure.
    await rm(sky, { force: true });

    engine = await SkyEngineE2e.start(mrp, {
      workDir: ws.dir,
      memory: "4M",
      dnsMap: LOCAL_DNS_MAP,
      timeoutMs: 90_000,
    });
    const started = await waitForStartedFrame(engine);

    expect(started.pixel(184, 128)).toEqual([240, 240, 240]);
    expect(started.uniqueColorCount()).toBe(1_697);
    expect((await stat(sky)).size).toBe(3_822_820);
    expect(await sha256(sky)).toBe(SKY_SHA256);

    const startedDrawCount = await engine.drawCount();
    expect(startedDrawCount).toBeGreaterThan(13);
    await vi.waitFor(async () => {
      expect(await engine!.drawCount()).toBeGreaterThan(startedDrawCount);
    }, { timeout: 5_000, interval: 100 });

    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(/UC_ERR|UC_MEM|unmapped|INVARIANT|uc_emu_start failed|FATAL/);
  }, 120_000);

  it("keeps a valid MRP identity when the imported host name exceeds 32 bytes", async () => {
    expect(await sha256("test/fixtures/op6120.mrp")).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    const importedName = "愤怒的小鸟VS僵尸2_v1002.mrp";
    // game.ext reads record[100] as a direct package-name pointer and checks
    // for `.mrp`; its earlier selector-100 MPS initializer must not be mistaken
    // for a fixed package-name copy merely because the basename is 34 bytes.
    expect(Buffer.byteLength(importedName, "utf8")).toBe(34);
    const mrp = ws.path(`mythroad/${importedName}`);
    await copyFile("test/fixtures/op6120.mrp", mrp);
    await copyFile("test/fixtures/plugins/advbar.mrp", ws.path("mythroad/plugins/advbar.mrp"));

    engine = await SkyEngineE2e.start(mrp, {
      workDir: ws.dir,
      dnsMap: LOCAL_DNS_MAP,
      timeoutMs: 90_000,
    });
    const started = await waitForStartedFrame(engine);

    expect(started.pixel(184, 128)).toEqual([240, 240, 240]);
    expect(started.uniqueColorCount()).toBe(1_697);
    expect(await engine.drawCount()).toBeGreaterThan(0);

    const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
    expect(output).not.toMatch(/UC_ERR|UC_MEM|unmapped|INVARIANT|uc_emu_start failed|FATAL/);
  }, 120_000);
});
