import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

describe("gtcm 进入主界面", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("游戏正常启动", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/gtcm'), { recursive: true, force: true })
    
    // gtcm 面向 320x480 竖屏真机,启动时经 plat(101,3) 请求横屏,
    // 模拟器窗口自动翻转为 480x320——断言坐标仍是横屏坐标。
    engine = await SkyEngineE2e.start("test/fixtures/gtcm.mrp", { workDir: ws.dir, screen: "320x480" });

    await engine.delay(7000);
    // 不开启音乐
    await engine.key('RIGHT_SOFT', 1_000);
    await engine.delay(2_000);
    {  
      await engine.key('ENTER', 1_000);
      await engine.delay(1_000);
      const screen = await engine.screen("home");
      // 白色云朵 rgb(248, 252, 248)
      expect(screen.pixel(344, 17)).toEqual([248, 252, 248]);
      // 标题"贪吃猫"黑色描边 rgb(0, 0, 0)
      expect(screen.pixel(280, 46)).toEqual([0, 0, 0]);
      // 灰色地面 rgb(128, 128, 128)
      // (背景蓝色射线区域是旋转动画,颜色随帧变化,不能用作断言点)
      expect(screen.pixel(20, 300)).toEqual([128, 128, 128]);
    }
    {
      // 开始游戏(进入场景会二次加载 UID.scene——大小写与安装时不同,
      // 依赖 dsm.c get_filename 的大小写不敏感回退)
      await engine.delay(1_000);
      await engine.key('ENTER', 1_000);
      await engine.delay(3_000);
      const screen = await engine.screen("game");
      // 白云 rgb(248, 252, 248)
      expect(screen.pixel(240, 120)).toEqual([248, 252, 248]);
      // 天空渐变中部 rgb(120, 192, 232)
      expect(screen.pixel(20, 160)).toEqual([120, 192, 232]);
      // 天空顶部 rgb(0, 140, 216)
      expect(screen.pixel(240, 10)).toEqual([0, 140, 216]);
    }
  });
  
  it("付费界面修正", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/gtcm'), { recursive: true, force: true })
    fs.rmSync(ws.path('mythroad/plugins/netpay.mrp'), { recursive: true, force: true })
    
    // gtcm 面向 320x480 竖屏真机,启动时经 plat(101,3) 请求横屏,
    // 模拟器窗口自动翻转为 480x320——断言坐标仍是横屏坐标。
    engine = await SkyEngineE2e.start("test/fixtures/gtcm.mrp", { workDir: ws.dir, screen: "320x480" });

    await engine.delay(7_000);
    // 不开启音乐
    await engine.key('RIGHT_SOFT', 1_000);
    await engine.delay(2_000);
    {  
      await engine.key('ENTER', 1_000);
      await engine.delay(1_000);
      const screen = await engine.screen("home");
      // 白色云朵 rgb(248, 252, 248)
      expect(screen.pixel(344, 17)).toEqual([248, 252, 248]);
      // 标题"贪吃猫"黑色描边 rgb(0, 0, 0)
      expect(screen.pixel(280, 46)).toEqual([0, 0, 0]);
      // 灰色地面 rgb(128, 128, 128)
      // (背景蓝色射线区域是旋转动画,颜色随帧变化,不能用作断言点)
      expect(screen.pixel(20, 300)).toEqual([128, 128, 128]);
    }
    {
      // 开始游戏(进入场景会二次加载 UID.scene——大小写与安装时不同,
      // 依赖 dsm.c get_filename 的大小写不敏感回退)
      await engine.delay(1_000);
      await engine.key('ENTER', 1_000);
      await engine.delay(3_000);
      const screen = await engine.screen("game");
      // 白云 rgb(248, 252, 248)
      expect(screen.pixel(240, 120)).toEqual([248, 252, 248]);
      // 天空渐变中部 rgb(120, 192, 232)
      expect(screen.pixel(20, 160)).toEqual([120, 192, 232]);
      // 天空顶部 rgb(0, 140, 216)
      expect(screen.pixel(240, 10)).toEqual([0, 140, 216]);
    }
    {
      await engine.waitForPixel(231, 88, [0, 0, 0], { name: "game-over", timeoutMs: 120_000 });
    }
    {
      await engine.key('ENTER', 1_000);
      await engine.delay(2_000);
      // netpay 下载提示界面:gtcm 进入付费流程前调 plat(101,0) 撤销 LCD
      // 旋转(真机行为,用户竖握手机操作),窗口自动翻回 320x480 竖屏。
      // 断言标题栏与底部软键栏的深灰底色 rgb(40, 40, 40)。
      const screen = await engine.screen("pay");
      expect(screen.pixel(60, 8)).toEqual([40, 40, 40]);
      expect(screen.pixel(80, 465)).toEqual([40, 40, 40]);
      expect(screen.pixel(240, 465)).toEqual([40, 40, 40]);
    }
  });
});
