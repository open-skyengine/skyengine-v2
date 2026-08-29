import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const ISTORE_OFFLINE_DNS_MAP = [
  "rop.skymobiapp.com->127.0.0.1",
  "spd.skymobiapp.com->127.0.0.1",
  "wap.skmeg.com->127.0.0.1",
].join(";");

describe("istore 进入主界面", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  async function openHome() {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    const activeEngine = await SkyEngineE2e.start("test/fixtures/sky_istore.mrp", {
      workDir: ws.dir,
      memory: "2M",
      dnsMap: ISTORE_OFFLINE_DNS_MAP,
    });
    engine = activeEngine;

    await activeEngine.waitForScreen(screen =>
      screen.pixel(123, 78).toString() === "112,152,208"
      && screen.pixel(143, 162).toString() === "216,220,216"
      && screen.pixel(121, 205).toString() === "168,208,80", {
        name: "network-error",
        timeoutMs: 30_000,
        intervalMs: 1_000,
      });

    await activeEngine.click(120, 215, 5_000);
    const home = await activeEngine.waitForScreen(screen =>
      screen.pixel(23, 291).toString() === "144,144,144"
      && screen.pixel(70, 291).toString() === "72,76,72", {
        name: "home",
        timeoutMs: 30_000,
        intervalMs: 500,
      });
    return { engine: activeEngine, home };
  }

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("确认网络错误后可以打开分类页", async () => {
    const { engine, home } = await openHome();

    await engine.delay(1_000);
    await engine.click(72, 305, 5_000);
    const category = await engine.waitForScreen(screen =>
      screen.pixel(70, 291).toString() === "32,32,32"
      && screen.pixel(23, 291).toString() === "72,76,72"
      && screen.pixel(116, 128).toString() === "216,220,216", {
        name: "category",
        timeoutMs: 90_000,
        intervalMs: 500,
      });

    const nav = { x: 0, y: 288, width: 96, height: 32 };
    expect(home.diffPixelCount(category, nav)).toBeGreaterThan(2_000);
    expect(await engine.waitForExit(1_000)).toBe(false);

    await engine.stop();
    const stderr = await readFile(engine.stderrPath, "utf8");
    expect(stderr).not.toMatch(
      /ARM fault|ABI error|unmapped|no memory|guest heap exhausted|panicked at/i,
    );
  }, 150_000);

  it("确认网络错误后可以打开搜索页", async () => {
    const { engine, home } = await openHome();

    await engine.delay(1_000);
    await engine.click(168, 305, 5_000);
    const search = await engine.waitForScreen(screen =>
      screen.pixel(166, 291).toString() === "32,32,32"
      && screen.pixel(23, 291).toString() === "72,76,72"
      && screen.pixel(200, 38).toString() === "112,152,208"
      && screen.pixel(10, 61).toString() === "216,220,216"
      && screen.pixel(10, 84).toString() === "224,228,224", {
        name: "search",
        timeoutMs: 30_000,
        intervalMs: 500,
      });

    const content = { x: 0, y: 25, width: 240, height: 263 };
    expect(home.diffPixelCount(search, content)).toBeGreaterThan(20_000);
    expect(await engine.waitForExit(1_000)).toBe(false);

    await engine.stop();
    const stderr = await readFile(engine.stderrPath, "utf8");
    expect(stderr).not.toMatch(
      /ARM fault|ABI error|unmapped|no memory|guest heap exhausted|panicked at/i,
    );
  }, 90_000);
});
