import { spawnSync } from "node:child_process";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { resolveSkyEngineBin, SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

describe("设备日期配置", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  let savedDeviceDate: string | undefined;

  beforeEach(() => {
    savedDeviceDate = process.env.SKYENGINE_DEVICE_DATE;
  });

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
    if (savedDeviceDate === undefined) delete process.env.SKYENGINE_DEVICE_DATE;
    else process.env.SKYENGINE_DEVICE_DATE = savedDeviceDate;
  });

  it.each([
    "2011-02-29",
    "1900-02-29",
    "2011-13-01",
    "2011-00-01",
    "2011-01-00",
    "2011-01-01x",
  ])("在启动前拒绝非法日期 %s", date => {
    const bin = resolveSkyEngineBin();
    const result = spawnSync(bin, [
      "--device-date",
      date,
      "test/fixtures/gtxzj.mrp",
    ], { encoding: "utf8" });

    // A missing Windows Release binary is a spawn failure, not evidence that the
    // emulator rejected the date; surface it before checking the process status.
    if (result.error) throw result.error;
    // main() returns -1; hosts encode that value differently, but all must
    // report a real nonzero exit together with the precise validation error.
    expect(result.status).not.toBeNull();
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain(`invalid device date '${date}'`);
  });

  it("同步启动遵循 VMRP_BIN 指定的 CI 产物路径", () => {
    const savedVmrpBin = process.env.VMRP_BIN;
    const ciArtifactBin = "build/skyengine.exe";
    try {
      process.env.VMRP_BIN = ciArtifactBin;
      expect(resolveSkyEngineBin()).toBe(ciArtifactBin);
    } finally {
      if (savedVmrpBin === undefined) delete process.env.VMRP_BIN;
      else process.env.VMRP_BIN = savedVmrpBin;
    }
  });

  it("接受环境变量中的确定性通过日期", async () => {
    process.env.SKYENGINE_DEVICE_DATE = "2012-06-20";
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/gtxzj.mrp", { workDir: ws.dir });

    await engine.delay(4_000);
    await engine.waitForPixel(79, 160, [248, 0, 0], {
      name: "env-date-sound-menu",
      timeoutMs: 10_000,
      intervalMs: 200,
    });
  }, 25_000);

  it("命令行日期覆盖无效环境变量", async () => {
    process.env.SKYENGINE_DEVICE_DATE = "invalid";
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/gtxzj.mrp", {
      workDir: ws.dir,
      deviceDate: "2012-06-20",
    });

    await engine.delay(4_000);
    await engine.waitForPixel(79, 160, [248, 0, 0], {
      name: "cli-date-sound-menu",
      timeoutMs: 10_000,
      intervalMs: 200,
    });
  }, 25_000);

  it("host 模式暴露当前宿主日期", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/gtxzj.mrp", {
      workDir: ws.dir,
      deviceDate: "host",
    });

    // 当前宿主年份晚于反汇编确认的 2012 门禁，初始化应在首帧前返回。
    await engine.delay(500);
    expect(await engine.drawCount()).toBe(0);
    expect((await engine.screen("host-date-black")).uniqueColorCount()).toBe(1);
  }, 10_000);
});
