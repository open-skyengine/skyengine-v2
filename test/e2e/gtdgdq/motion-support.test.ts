import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";

function paddleX(screen: Awaited<ReturnType<SkyEngineE2e["screen"]>>): number | undefined {
  const positions: number[] = [];
  for (let y = 260; y < 300; y += 1) {
    for (let x = 0; x < screen.width; x += 1) {
      const pixel = screen.pixel(x, y);
      if (pixel[0] === 24 && pixel[1] === 120 && pixel[2] === 248) positions.push(x);
    }
  }
  if (positions.length === 0) return undefined;
  return positions.reduce((sum, x) => sum + x, 0) / positions.length;
}

describe("gtdgdq", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("传感器检测", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/gtdgdq'), { recursive: true, force: true })
    
    // gtcm 面向 320x480 竖屏真机,启动时经 plat(101,3) 请求横屏,
    // 模拟器窗口自动翻转为 480x320——断言坐标仍是横屏坐标。
    engine = await SkyEngineE2e.start("test/fixtures/gtdgdq.mrp", { workDir: ws.dir });

    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error('skyengine not defined')
        const screen = await engine.screen("bgm-select");
        // rgb(0, 200, 248)
        expect(screen.pixel(219, 312)).toEqual([0, 200, 248]);
      })
      // 不开启音乐
      await engine.key('RIGHT_SOFT', 1_000);
    }
    {
      await vi.waitFor(async () => {
        if (!engine) throw new Error('skyengine not defined')
        const screen = await engine.screen("menu");
        // rgb(248, 248, 240)
        expect(screen.pixel(168, 162)).toEqual([248, 248, 240]);
      })
      await engine.key('LEFT_SOFT', 1_000);
    }
    {
      // 支持动感功能，只有两种颜色提示（黑色背景+绿色文字）
      await vi.waitFor(async () => {
        if (!engine) throw new Error('skyengine not defined')
        const screen = await engine.screen("menu");
        // rgb(248, 248, 240)
        expect(screen.uniqueColorCount()).toEqual(2);
      })
      await engine.key('LEFT_SOFT', 1_000);
      await engine.key('DOWN', 1_000);
      await engine.key('LEFT_SOFT', 1_000);
      await engine.waitForPixel(120, 160, [152, 40, 176], {
        name: "level-one-prompt",
        timeoutMs: 10_000,
        intervalMs: 250,
      });
      await engine.key('SELECT', 1_000);
      const before = await engine.waitForScreen(
        screen => screen.pixel(120, 160).join(",") !== "152,40,176",
        { name: "game-before-motion", timeoutMs: 10_000 }
      );
      await engine.delay(250);
      const neutral = await engine.screen("game-neutral");
      const beforeX = paddleX(before);
      const neutralX = paddleX(neutral);
      expect(beforeX).toBeDefined();
      expect(neutralX).toBeDefined();
      expect(neutralX).toBe(beforeX);
      for (let sample = 0; sample < 5; sample += 1) {
        await engine.motion(100, 0, 0);
        await engine.delay(50);
      }
      const after = await engine.waitForScreen(
        screen => {
          const x = paddleX(screen);
          return x !== undefined && neutralX !== undefined && Math.abs(x - neutralX) > 5;
        },
        { name: "game-after-motion", timeoutMs: 2_000, intervalMs: 50 }
      );
      expect(Math.abs(paddleX(after)! - neutralX!)).toBeGreaterThan(5);
    }
  });

  it("取消使用动感功能后继续游戏", async () => {
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path("mythroad/gtdgdq"), { recursive: true, force: true });
    engine = await SkyEngineE2e.start("test/fixtures/gtdgdq.mrp", { workDir: ws.dir });

    await engine.waitForPixel(219, 312, [0, 200, 248], {
      name: "cancel-motion-bgm-select",
      timeoutMs: 10_000,
      intervalMs: 250,
    });
    await engine.key("RIGHT_SOFT", 1_000);
    await engine.waitForPixel(168, 162, [248, 248, 240], {
      name: "cancel-motion-menu",
      timeoutMs: 10_000,
      intervalMs: 250,
    });

    await engine.key("LEFT_SOFT", 1_000);
    await engine.waitForScreen(screen => screen.uniqueColorCount() === 2, {
      name: "cancel-motion-prompt",
      timeoutMs: 10_000,
      intervalMs: 250,
    });
    await engine.key("RIGHT_SOFT", 1_000);

    await engine.key("DOWN", 1_000);
    await engine.key("LEFT_SOFT", 1_000);
    await engine.waitForPixel(120, 160, [152, 40, 176], {
      name: "cancel-motion-level-one-prompt",
      timeoutMs: 10_000,
      intervalMs: 250,
    });
    await engine.key("SELECT", 1_000);
    const game = await engine.waitForScreen(
      screen => screen.pixel(120, 160).join(",") !== "152,40,176" && paddleX(screen) !== undefined,
      { name: "cancel-motion-game", timeoutMs: 10_000, intervalMs: 250 },
    );

    const paddleBeforeMotion = paddleX(game);
    expect(paddleBeforeMotion).toBeDefined();
    for (let sample = 0; sample < 5; sample += 1) {
      await engine.motion(100, 0, 0);
      await engine.delay(50);
    }
    const afterMotion = await engine.screen("cancel-motion-game-after-motion");
    expect(paddleX(afterMotion)).toBe(paddleBeforeMotion);
    expect(await engine.waitForExit(250)).toBe(false);
  });
  
});
