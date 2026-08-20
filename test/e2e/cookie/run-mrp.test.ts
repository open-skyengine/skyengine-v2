import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage, type Rgb } from "../engine-e2e.js";
import { cpSync, mkdirSync, rmSync } from "fs";

function pixelEquals(screen: PpmImage, x: number, y: number, expected: Rgb): boolean {
  const actual = screen.pixel(x, y);
  return actual[0] === expected[0] && actual[1] === expected[1] && actual[2] === expected[2];
}

function isOpeningFolderFrame(screen: PpmImage): boolean {
  return pixelEquals(screen, 194, 249, [24, 96, 216])
    && pixelEquals(screen, 150, 251, [0, 76, 200])
    && pixelEquals(screen, 100, 216, [248, 252, 248])
    && pixelEquals(screen, 8, 259, [232, 240, 248])
    && pixelEquals(screen, 42, 269, [232, 240, 248]);
}

function isStartupOverlayFrame(screen: PpmImage): boolean {
  return pixelEquals(screen, 194, 249, [24, 96, 216])
    && pixelEquals(screen, 150, 251, [0, 76, 200])
    && pixelEquals(screen, 100, 216, [248, 200, 24]);
}

function hasBottomFileHighlight(screen: PpmImage): boolean {
  // Runtime-created root entries can move the final item between the last two
  // visible rows.  The following launch and return-frame checks still prove
  // that this highlighted item is the browser package.
  for (let y = 232; y <= 284; y += 1) {
    if (pixelEquals(screen, 100, y, [248, 200, 24])) return true;
  }
  return false;
}

async function collectDrawFrames(
  engine: SkyEngineE2e,
  startDrawCount: number,
  namePrefix: string,
): Promise<PpmImage[]> {
  const frames: PpmImage[] = [];
  let drawCount = startDrawCount;
  for (let i = 0; i < 40; i += 1) {
    let response: string;
    try {
      response = await engine.command(`WAIT_DRAW ${drawCount} 250`, 750);
    } catch (error) {
      // A quiet 250ms interval is expected while a restarted parent rebuilds
      // its list.  Socket/protocol failures are not an end-of-frames signal.
      if (error instanceof Error && error.message.includes("wait_draw_timeout")) {
        continue;
      }
      throw error;
    }
    const match = /^OK draw_count (\d+)$/.exec(response);
    if (!match) throw new Error(`Unexpected WAIT_DRAW response: ${response}`);
    const nextDrawCount = Number(match[1]);
    for (let draw = drawCount + 1; draw <= nextDrawCount; draw += 1) {
      frames.push(await engine.screenDraw(draw, `${namePrefix}-${String(draw).padStart(4, "0")}`));
    }
    drawCount = nextDrawCount;
  }
  return frames;
}

describe("cookie 应用正常启动并且退出到文件管理器", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("方式一", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/dsm_gm.mrp"), { force: true, recursive: true });
    cpSync("test/fixtures/wbrw.mrp", ws.path("mythroad/wbrw.mrp"));
    // wbrw 首次运行会在根目录创建自己的数据目录 brw/。预创建它,使文件管理器
    // 的根目录列表在启动子应用前后保持一致,"返回后画面与启动前逐字节一致"
    // 的断言才有意义(否则子应用合法新增的目录会使列表项位移)。
    mkdirSync(ws.path("mythroad/brw"), { recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/cookie_v6110.mrp", { workDir: ws.dir, memory: "2M" });
    let fileManagerBeforeLaunch: PpmImage | undefined;

    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 从第一项向上循环到末项“文件管理”，直接验证末行高亮背景。
        const screen = await engine.screen("home");
        // rgb(32, 72, 112)
        expect(screen.pixel(75, 276)).toEqual([32, 72, 112]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('LEFT_SOFT', 1_000, 250)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 主页面
        const screen = await engine.screen("menu-first");
        // rgb(248, 192, 32)
        expect(screen.pixel(94, 166)).toEqual([248, 192, 32]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('UP', 1_000)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 主页面
        const screen = await engine.screen("menu-last");
        // rgb(248, 192, 32)
        expect(screen.pixel(99, 274)).toEqual([248, 192, 32]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('LEFT_SOFT', 1_000)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 文件管理根目录加载完成后第一项高亮；目录可全部显示时右侧不会绘制滚动滑块。
        const screen = await engine.screen("file-manage-top");
        // rgb(248, 200, 24)
        expect(screen.pixel(100, 48)).toEqual([248, 200, 24]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('UP', 1_000)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 从第一项向上循环到末项“冒泡浏览器”，验证底部文件行高亮。
        const screen = await engine.screen("file-manage-bottom");
        expect(hasBottomFileHighlight(screen)).toBe(true);
        fileManagerBeforeLaunch = screen;
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('LEFT_SOFT', 1_000)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 文件菜单第一项“进入”的高亮背景证明菜单已打开。
        const screen = await engine.screen("file-manage-menu");
        // rgb(248, 192, 32)
        expect(screen.pixel(86, 251)).toEqual([248, 192, 32]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      // 启动
      await engine.key('LEFT_SOFT', 1_000)

      // rgb(200, 228, 248)
      const secondApp = await engine.waitForPixel(
        211,
        88,
        [200, 228, 248],
        {
          name: "second-app",
          timeoutMs: 10_000,
          intervalMs: 250,
        },
      );
      // rgb(248, 252, 248)
      expect(secondApp.pixel(150, 251)).toEqual([248, 252, 248]);
    }
    {
      // 打开菜单
      await engine.key('LEFT_SOFT', 1_000)
      // rgb(224, 240, 248)
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        const screen = await engine.screen("second-app-menu");
        expect(screen.pixel(71, 283)).toEqual([224, 240, 248]);
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 向上选择最后一个菜单项“退出”
      await engine.key('UP', 1_000)
      // rgb(128, 192, 240)
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        const screen = await engine.screen("second-app-menu");
        expect(screen.pixel(68, 284)).toEqual([128, 192, 240]);
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 选择退出
      await engine.delay(1_000)
      await engine.key('LEFT_SOFT', 1_000)
      // 二次确认退出
      await engine.delay(1_000)
      // 退出，返回文件管理器，跟file-manage-bottom画面一致
      const returnDrawCount = await engine.drawCount();
      await engine.key('LEFT_SOFT', { waitForDraw: false })
      const returnFrames = await collectDrawFrames(engine, returnDrawCount, "return-draw");

      expect(returnFrames.some(isOpeningFolderFrame)).toBe(true);
      expect(returnFrames.some(isStartupOverlayFrame)).toBe(false);

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        if (!fileManagerBeforeLaunch) throw new Error("启动前文件管理器画面未初始化");
        const screen = await engine.screen("back-to-file-manage");
        expect(screen.diffPixelCount(fileManagerBeforeLaunch, {
          x: 0,
          y: 0,
          width: 240,
          height: 296,
        })).toBe(0);
      }, {
        timeout: 10_000,
        interval: 1000
      })

      // 文件管理器的“退出”先返回 Cookie 主界面；恢复后的父 VM 必须继续
      // 接收正常事件，不能只停留在一张宿主强制提交的列表快照上。
      await engine.key('RIGHT_SOFT', 2_000);
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        const screen = await engine.screen("parent-home-after-return");
        expect(screen.pixel(75, 276)).toEqual([32, 72, 112]);
      }, {
        timeout: 10_000,
        interval: 1_000,
      });
    }
  });

  it("方式二", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    rmSync(ws.path("mythroad/dsm_gm.mrp"), { force: true, recursive: true });
    cpSync("test/fixtures/wbrw.mrp", ws.path("mythroad/wbrw.mrp"));
    // wbrw 首次运行会在根目录创建自己的数据目录 brw/。预创建它,使文件管理器
    // 的根目录列表在启动子应用前后保持一致,"返回后画面与启动前逐字节一致"
    // 的断言才有意义(否则子应用合法新增的目录会使列表项位移)。
    mkdirSync(ws.path("mythroad/brw"), { recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/cookie_v6110.mrp", { workDir: ws.dir, memory: "2M" });
    let fileManagerBeforeLaunch: PpmImage | undefined;

    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 从第一项向上循环到末项“文件管理”，直接验证末行高亮背景。
        const screen = await engine.screen("home");
        // rgb(32, 72, 112)
        expect(screen.pixel(75, 276)).toEqual([32, 72, 112]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('LEFT_SOFT', 1_000, 250)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 主页面
        const screen = await engine.screen("menu-first");
        // rgb(248, 192, 32)
        expect(screen.pixel(94, 166)).toEqual([248, 192, 32]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('UP', 1_000)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 主页面
        const screen = await engine.screen("menu-last");
        // rgb(248, 192, 32)
        expect(screen.pixel(99, 274)).toEqual([248, 192, 32]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('LEFT_SOFT', 1_000)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 文件管理根目录加载完成后第一项高亮；目录可全部显示时右侧不会绘制滚动滑块。
        const screen = await engine.screen("file-manage-top");
        // rgb(248, 200, 24)
        expect(screen.pixel(100, 48)).toEqual([248, 200, 24]);
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      await engine.key('UP', 1_000)

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        // 从第一项向上循环到末项“冒泡浏览器”，验证底部文件行高亮。
        const screen = await engine.screen("file-manage-bottom");
        expect(hasBottomFileHighlight(screen)).toBe(true);
        fileManagerBeforeLaunch = screen;
      }, {
        timeout: 10_000,
        interval: 1000
      })
    }
    {
      // 启动
      await engine.key('ENTER', 1_000)

      // rgb(200, 228, 248)
      const secondApp = await engine.waitForPixel(
        211,
        88,
        [200, 228, 248],
        {
          name: "second-app",
          timeoutMs: 10_000,
          intervalMs: 250,
        },
      );
      // rgb(248, 252, 248)
      expect(secondApp.pixel(150, 251)).toEqual([248, 252, 248]);
    }
    {
      // 打开菜单
      await engine.key('LEFT_SOFT', 1_000)
      // rgb(224, 240, 248)
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        const screen = await engine.screen("second-app-menu");
        expect(screen.pixel(71, 283)).toEqual([224, 240, 248]);
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 向上选择最后一个菜单项“退出”
      await engine.key('UP', 1_000)
      // rgb(128, 192, 240)
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        const screen = await engine.screen("second-app-menu");
        expect(screen.pixel(68, 284)).toEqual([128, 192, 240]);
      }, {
        timeout: 10_000,
        interval: 1_000
      })
    }
    {
      // 选择退出
      await engine.delay(1_000)
      await engine.key('LEFT_SOFT', 1_000)
      // 二次确认退出
      await engine.delay(1_000)
      // 退出，返回文件管理器，跟file-manage-bottom画面一致
      const returnDrawCount = await engine.drawCount();
      await engine.key('LEFT_SOFT', { waitForDraw: false })
      const returnFrames = await collectDrawFrames(engine, returnDrawCount, "return-draw");

      expect(returnFrames.some(isOpeningFolderFrame)).toBe(true);
      expect(returnFrames.some(isStartupOverlayFrame)).toBe(false);

      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        if (!fileManagerBeforeLaunch) throw new Error("启动前文件管理器画面未初始化");
        const screen = await engine.screen("back-to-file-manage");
        expect(screen.diffPixelCount(fileManagerBeforeLaunch, {
          x: 0,
          y: 0,
          width: 240,
          height: 296,
        })).toBe(0);
      }, {
        timeout: 10_000,
        interval: 1000
      })

      // 文件管理器的“退出”先返回 Cookie 主界面；恢复后的父 VM 必须继续
      // 接收正常事件，不能只停留在一张宿主强制提交的列表快照上。
      await engine.key('RIGHT_SOFT', 2_000);
      await vi.waitFor(async () => {
        if (!engine) throw new Error("skyengine 未初始化");
        const screen = await engine.screen("parent-home-after-return");
        expect(screen.pixel(75, 276)).toEqual([32, 72, 112]);
      }, {
        timeout: 10_000,
        interval: 1_000,
      });
    }
  });
});
