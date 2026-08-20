import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("opglqa 进入主菜单", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  const expectMainScreen = async (name: string) => {
    for (let i = 0; i < 8; i++) {
      const screen = await engine!.screen(name);
      // rgb(144, 212, 248)
      if (screen.pixel(9, 188).join(",") === "144,212,248") {
        return;
      }
      // Font installation briefly presents a black transition frame after the
      // success dialog closes; wait for the first stable main-screen paint.
      await engine!.delay(500);
    }
    const screen = await engine!.screen(name);
    // rgb(144, 212, 248)
    expect(screen.pixel(9, 188)).toEqual([144, 212, 248]);
  };

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("程序准备 - 顶部正常", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 删除后，继续游戏会进入下载netpay插件界面。
    if (!fs.existsSync(ws.path('mythroad/plugins/netpay.mrp'))) {
      fs.cpSync('test/fixtures/plugins/netpay.mrp', ws.path('mythroad/plugins/netpay.mrp'));
    }
    // 本用例验证前台 advbar 插件与游戏主画面的屏幕合成，运行前显式准备插件资源。
    if (!fs.existsSync(ws.path('mythroad/plugins/advbar.mrp'))) {
      fs.cpSync('test/fixtures/plugins/advbar.mrp', ws.path('mythroad/plugins/advbar.mrp'));
    }
    // Start from the first-run font state; this test then verifies the second
    // launch reuses the files created by the first launch.
    fs.rmSync(ws.path('mythroad/system/gb12v2.uc2'), { force: true });
    fs.rmSync(ws.path('mythroad/system/gb12v2.adl'), { force: true });
    fs.rmSync(ws.path('mythroad/system/___sftmp.mrp'), { force: true });
    fs.rmSync(ws.path('mythroad/vld/77ae410'), { force: true, recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/opglqa_v4450.mrp", { workDir: ws.dir });
    {
      await engine.delay(1_000);
      const screen = await engine.screen("download-font");
      // rgb(40, 176, 216)
      expect(screen.pixel(87, 295)).toEqual([40, 176, 216]);
    }
    {
      // 开始下载字体资源，下载完成后显示下载完成结果。
      await engine.key('LEFT_SOFT', 1_000)
      await vi.waitFor(async () => {
        if (!engine) throw new Error("engine is undefined");
        const screen = await engine.screen("confirm-result");
        // rgb(0, 0, 0)
        expect(screen.pixel(155, 153)).toEqual([0, 0, 0]);
        // rgb(248, 252, 248)
        expect(screen.pixel(30, 301)).toEqual([248, 252, 248]);
      }, { timeout: 20_000, interval: 1_000 });
    }
    {
      // 确认下载结果，进入主界面。
      await engine.key('LEFT_SOFT', 1_000)
      await expectMainScreen("main-screen1");
    }

    await engine.close();
    await engine.delay(1_000);
    engine = await SkyEngineE2e.start("test/fixtures/opglqa_v4450.mrp", { workDir: ws.dir });
    {
      // 直接进入主界面。
      await expectMainScreen("main-screen2");
    }
  });
});
