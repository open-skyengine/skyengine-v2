import { createHash } from "node:crypto";
import { copyFile, readFile, rm, stat } from "node:fs/promises";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage } from "../engine-e2e.js";

const MRP_SHA256 = "8a0d57d6952412cb9bf7ca21766b0a7831ccf8de90aded3cebef2bcb5b32c0e8";
const SKY_SHA256 = "2d2708d82c46ef5a830b9f9635bd60b4e21e795fa6cf9be9e8dc6850615e9565";
const AVM_SHA256 = "d9d91908442aad3e766ba357bfa5f1a430a5bb9baf691a2db5b054d1f75d572f";
const TITLE_SHA256 = "098e2b474feb2f9217d5bbf87b6fbf50e8bf088c13e2b82d15ce39a4fd5536cb";
const EXIT_SELECTED_SHA256 = "2e34259c05bbc975e8baef15749afd68b3c32f7fa4d8dad9ef2047d1ffbb3b88";

async function sha256(file: string): Promise<string> {
  return createHash("sha256").update(await readFile(file)).digest("hex");
}

async function waitForFrame(
  engine: SkyEngineE2e,
  name: string,
  expectedHash: string,
  timeoutMs: number,
): Promise<PpmImage> {
  let frame: PpmImage | undefined;
  await vi.waitFor(async () => {
    frame = await engine.screen(name);
    expect(await sha256(path.join(engine.tmpDir, `${name}.ppm`))).toBe(expectedHash);
  }, { timeout: timeoutMs, interval: 500 });
  if (!frame) throw new Error(`Frame ${name} was not captured`);
  return frame;
}

async function expectNoArmFault(engine: SkyEngineE2e): Promise<void> {
  const output = `${await readFile(engine.stdoutPath, "utf8")}\n${await readFile(engine.stderrPath, "utf8")}`;
  expect(output).not.toMatch(/UC_MEM|UC_ERR|uc_emu_start failed|FATAL/);
}

// 功夫熊猫大战(240x320, cfunction.ext wrapper + flaengine.ext + generated SKY/AVM)。
// 回归边界同时覆盖 table[125] nested owner、32-bit wrapped LG_mem first-fit、
// 冷启动缓存生成，以及历史坏 SKY 所表现的加载后近全黑画面。
describe("wxmdld 冷暖启动", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("生成完整 SKY/AVM 并进入可交互标题画面", async () => {
    expect(await sha256("test/fixtures/wxmdld.mrp")).toBe(MRP_SHA256);

    ws = await SkyEngineWorkspace.create();
    const mrp = ws.path("mythroad/wxmdld.mrp");
    const sky = ws.path("mythroad/cache/wxmdld/sky.bmp");
    const avm = ws.path("mythroad/avm_cache");
    await copyFile("test/fixtures/wxmdld.mrp", mrp);
    await rm(sky, { force: true });
    await rm(avm, { force: true });

    // 冷启动必须真正完成资源转换，不能依赖仓库或其它并发用例留下的缓存。
    engine = await SkyEngineE2e.start(mrp, { workDir: ws.dir, timeoutMs: 90_000 });
    const coldTitle = await waitForFrame(engine, "cold-title", TITLE_SHA256, 90_000);
    expect(coldTitle.uniqueColorCount()).toBe(196);
    expect(coldTitle.pixel(20, 20)).toEqual([224, 220, 216]);
    expect(coldTitle.pixel(120, 160)).toEqual([168, 164, 152]);
    expect((await stat(sky)).size).toBe(1_196_230);
    expect((await stat(avm)).size).toBe(219_848);
    expect(await sha256(sky)).toBe(SKY_SHA256);
    expect(await sha256(avm)).toBe(AVM_SHA256);
    await expectNoArmFault(engine);
    await engine.close();
    engine = undefined;

    // 同目录暖启动必须复用刚生成的文件，并仍落在完全相同的静态标题帧。
    engine = await SkyEngineE2e.start(mrp, { workDir: ws.dir, timeoutMs: 30_000 });
    const warmTitle = await waitForFrame(engine, "warm-title", TITLE_SHA256, 30_000);
    expect(coldTitle.diffPixelCount(warmTitle)).toBe(0);

    // DOWN/UP 只改变底部两个菜单项。既验证真实按键和 draw 推进，也防止
    // “进程活着但 SKY 图像区全零”的近黑画面再次被当成启动成功。
    const beforeDown = await engine.drawCount();
    await engine.key("DOWN", 5_000);
    expect(await engine.drawCount()).toBeGreaterThan(beforeDown);
    const exitSelected = await engine.screen("exit-selected");
    expect(await sha256(path.join(engine.tmpDir, "exit-selected.ppm"))).toBe(EXIT_SELECTED_SHA256);
    expect(warmTitle.diffPixelCount(exitSelected)).toBe(1_138);
    expect(warmTitle.diffPixelCount(exitSelected, {
      x: 84,
      y: 239,
      width: 71,
      height: 46,
    })).toBe(1_138);

    const beforeUp = await engine.drawCount();
    await engine.key("UP", 5_000);
    expect(await engine.drawCount()).toBeGreaterThan(beforeUp);
    const restored = await engine.screen("restored-title");
    expect(warmTitle.diffPixelCount(restored)).toBe(0);
    expect(await sha256(path.join(engine.tmpDir, "restored-title.ppm"))).toBe(TITLE_SHA256);
    expect(await sha256(sky)).toBe(SKY_SHA256);
    expect(await sha256(avm)).toBe(AVM_SHA256);
    await expectNoArmFault(engine);
  }, 180_000);
});
