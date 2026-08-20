import { createHash } from "node:crypto";
import { cpSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

type GjxqyServer = {
  completed: Promise<{
    requestLength: number;
    resourceLength: number;
    responseBodyLength: number;
    frameLengths: number[];
  }>;
  close(): Promise<void>;
};

const require = createRequire(import.meta.url);
const { startGjxqyServer } = require("../../../tool/jxqy-server.js") as {
  startGjxqyServer(options?: { resourcePath?: string }): Promise<GjxqyServer>;
};

// The original temp package is preserved; this fixture only changes fileid.bin
// to the wrapper's disassembled slot-1 ID so the installer can complete.
const DERIVED_RESOURCE_PATH = "test/fixtures/gjxqy-derived/res_1.mrp";
const DERIVED_RESOURCE_SIZE = 273_877;
const DERIVED_RESOURCE_SHA256 = "94ebbc1cb0b2c0dea74abe0bcf8b656b9ba3a5eb3d10c83b08c3ae1232fe9448";
const RESOURCE_1_SIZE = 1_294_042;
const RESOURCE_1_SHA256 = "11c0543e9e213f635b59ae9dee989078bac87f94257c306a3dc5a34f16abba07";
const INSTALLED_RES_V = "f0030000010000008d9b5fa4";

describe("gjxqy", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  let server: GjxqyServer | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await server?.close();
    server = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("renders the outdoor and tavern scenes", async () => {
    ws = await SkyEngineWorkspace.create();
    // The app expects its extracted resource index under mythroad/gjxqy.
    cpSync("test/fixtures/gjxqy", ws.path("mythroad/gjxqy"), { recursive: true });
    // res.v marks slot 1 as installed, so the fixture must contain the assembled
    // res.bin1 produced by that installation, not only its unpacked res.p* parts.
    const resource1 = await readFile(ws.path("mythroad/gjxqy/res.bin1"));
    expect(resource1).toHaveLength(RESOURCE_1_SIZE);
    expect(createHash("sha256").update(resource1).digest("hex")).toBe(RESOURCE_1_SHA256);

    engine = await SkyEngineE2e.start("test/fixtures/gjxqy.mrp", {
      workDir: ws.dir,
      dnsMap: "211.155.236.18->127.0.0.1"
    });

    await vi.waitFor(async () => {
      const screen = await engine!.screen("bgm-select");
      expect(screen.pixel(154, 145)).toEqual([248, 252, 0]);
    }, { timeout: 10_000, interval: 1_000 });

    // Keep music disabled so audio timing cannot change the network transition.
    await engine.key("RIGHT_SOFT", 1_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("menu");
      expect(screen.pixel(119, 289)).toEqual([248, 248, 248]);
    }, { timeout: 10_000, interval: 1_000 });

    await engine.key("RIGHT", 1_000);
    await engine.delay(1_000);
    await engine.key("ENTER", 1_000);
    await engine.delay(1_000);
    await engine.key("ENTER", 1_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("scene");
      expect(screen.pixel(152, 222)).toEqual([160, 188, 192]);
    }, { timeout: 10_000, interval: 1_000 });

    await vi.waitFor(async () => {
      await engine!.key("UP", 1_000, 1_000);
      const screen = await engine!.screen("scene-next");
      expect(screen.pixel(152, 222)).toEqual([0, 0, 0]);
    }, { timeout: 30_000, interval: 1_000 });

    await vi.waitFor(async () => {
      const screen = await engine!.screen("scene-next");
      // These points cover the tavern walls, furniture and floor. The broken
      // fixture leaves all of them black and renders only the centered name tile.
      expect(screen.pixel(10, 60)).toEqual([248, 248, 224]);
      expect(screen.pixel(60, 100)).toEqual([152, 104, 48]);
      expect(screen.pixel(152, 222)).toEqual([224, 220, 200]);
      expect(screen.pixel(220, 300)).toEqual([248, 228, 104]);
      expect(screen.uniqueColorCount()).toBeGreaterThan(200);
    }, { timeout: 10_000, interval: 1_000 });
  });
  it("download res", async () => {
    ws = await SkyEngineWorkspace.create();
    const derivedResource = await readFile(DERIVED_RESOURCE_PATH);
    expect(derivedResource).toHaveLength(DERIVED_RESOURCE_SIZE);
    expect(createHash("sha256").update(derivedResource).digest("hex")).toBe(DERIVED_RESOURCE_SHA256);

    server = await startGjxqyServer({ resourcePath: DERIVED_RESOURCE_PATH });

    engine = await SkyEngineE2e.start("test/fixtures/gjxqy.mrp", {
      workDir: ws.dir,
      dnsMap: "211.155.236.18->127.0.0.1"
    });

    await vi.waitFor(async () => {
      const screen = await engine!.screen("bgm-select");
      expect(screen.pixel(154, 145)).toEqual([248, 252, 0]);
    }, { timeout: 10_000, interval: 1_000 });

    // Keep music disabled so audio timing cannot change the network transition.
    await engine.key("RIGHT_SOFT", 1_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("menu");
      expect(screen.pixel(119, 289)).toEqual([248, 248, 248]);
    }, { timeout: 10_000, interval: 1_000 });

    await engine.key("RIGHT", 1_000);
    await engine.delay(1_000);
    await engine.key("ENTER", 1_000);

    await expect(server.completed).resolves.toEqual({
      requestLength: 161,
      resourceLength: DERIVED_RESOURCE_SIZE,
      responseBodyLength: 273_959,
      frameLengths: [102, 44, 5, 2, 31, DERIVED_RESOURCE_SIZE],
    });

    await vi.waitFor(async () => {
      const resource1 = await readFile(ws!.path("mythroad/gjxqy/res.bin1"));
      expect(resource1).toHaveLength(RESOURCE_1_SIZE);
      expect(createHash("sha256").update(resource1).digest("hex")).toBe(RESOURCE_1_SHA256);
      const resVersion = await readFile(ws!.path("mythroad/gjxqy/res.v"));
      expect(resVersion.toString("hex")).toBe(INSTALLED_RES_V);
    }, { timeout: 20_000, interval: 1_000 });

    await vi.waitFor(async () => {
      const screen = await engine!.screen("download-installed");
      expect(screen.pixel(10, 60)).toEqual([192, 220, 248]);
      expect(screen.pixel(60, 100)).toEqual([192, 224, 248]);
      expect(screen.pixel(152, 222)).toEqual([120, 144, 144]);
      expect(screen.uniqueColorCount()).toBeGreaterThan(40);
    }, { timeout: 10_000, interval: 1_000 });
  });
});
