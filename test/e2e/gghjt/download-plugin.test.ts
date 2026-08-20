import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs, { cpSync } from "fs";

describe("gghjt pixel flow", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  const memCheckTime = 10_000

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
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
    // 等待下载开始
    await engine.delay(5_000);
    // 检查屏幕是否成功的绿色
    const downloading = await engine.screen("download-ing");
    expect(downloading.pixel(70, 176)).toEqual([0, 252, 0]);
    // 等待下载完成
    await engine.delay(10_000);
    // 检查屏幕是否成功的绿色
    const success = await engine.screen("download-end");
    expect(success.pixel(101, 148)).toEqual([0, 252, 0]);
  
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
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", { workDir: ws.dir });

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
    // 等待下载开始
    await vi.waitFor(async () => {
      if (!engine) throw new Error('skyengine not defined')
      // 检查屏幕是否成功的绿色
      const downloading = await engine.screen("download-ing");
      expect(downloading.pixel(70, 176)).toEqual([0, 252, 0]);
    }, {
      timeout: 20_000,
      interval: 1_000
    })
    // 等待下载完成
    await vi.waitFor(async () => {
      if (!engine) throw new Error('skyengine not defined')
      // 检查屏幕是否成功的绿色
      const success = await engine.screen("download-end");
      expect(success.pixel(101, 148)).toEqual([0, 252, 0]);
    }, {
      timeout: 20_000,
      interval: 1_000
    })

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
    engine = await SkyEngineE2e.start("test/fixtures/gghjt.mrp", {
      workDir: ws.dir,
      dnsMap: 'rop.skymobiapp.com->127.0.0.1;spd.skymobiapp.com->159.75.119.124'
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
    // 等待下载开始
    await engine.delay(5_000);
    // 检查屏幕是否成功的绿色
    const downloading = await engine.screen("download-ing");
    expect(downloading.pixel(70, 176)).toEqual([0, 252, 0]);
    // 等待下载完成
    await engine.delay(10_000);
    // 检查屏幕是否成功的绿色
    const success = await engine.screen("download-end");
    expect(success.pixel(101, 148)).toEqual([0, 252, 0]);
  
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
