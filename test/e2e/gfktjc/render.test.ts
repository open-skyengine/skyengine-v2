import { afterEach, describe, expect, it, vi } from "vitest";
import { type PpmImage, SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

function countColor(
  image: PpmImage,
  color: readonly [number, number, number],
  rect: { x: number; y: number; width: number; height: number },
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      const pixel = image.pixel(x, y);
      if (pixel[0] === color[0] && pixel[1] === color[1] && pixel[2] === color[2]) count++;
    }
  }
  return count;
}
import fs from "fs";

describe("gfktjc", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("初始渲染检测", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/gfktjc-v1005.mrp", { workDir: ws.dir });

    {
      // 等待进入图形加速提示界面
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("main");
          // rgb(240, 180, 64)
          expect(screen.pixel(116, 62)).toEqual([240, 180, 64]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
    {
      // 不启用图形加速
      await engine.key("RIGHT_SOFT", 1_000);
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("bgm-select");
          // rgb(240, 180, 64)
          expect(screen.pixel(116, 62)).not.toEqual([240, 180, 64]);
          // rgb(240, 180, 64)
          expect(screen.pixel(111, 132)).toEqual([240, 180, 64]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
  });
  it("游戏内渲染检测", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.cpSync("test/fixtures/gfktjc", ws.path("mythroad/gfktjc"), { recursive: true });
    engine = await SkyEngineE2e.start("test/fixtures/gfktjc-v1005.mrp", { workDir: ws.dir });

    {
      // 等待进入图形加速提示界面
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("main");
          // rgb(240, 180, 64)
          expect(screen.pixel(116, 62)).toEqual([240, 180, 64]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
    {
      // 不启用图形加速
      await engine.key("RIGHT_SOFT", 1_000);
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("bgm-select");
          // rgb(240, 180, 64)
          expect(screen.pixel(116, 62)).not.toEqual([240, 180, 64]);
          // rgb(240, 180, 64)
          expect(screen.pixel(111, 132)).toEqual([240, 180, 64]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
    {
      // 不开启音乐
      await engine.key("RIGHT_SOFT", 1_000);
      const menu = await engine.waitForScreen(
        screen =>
          screen.pixel(218, 186).toString() === "0,0,0" &&
          screen.pixel(153, 298).toString() === "248,140,0",
        { name: "game-menu", timeoutMs: 20_000, intervalMs: 250 },
      );
      expect(menu.pixel(218, 186)).toEqual([0, 0, 0]);
      expect(menu.pixel(153, 298)).toEqual([248, 140, 0]);
    }
    {
      // 选择开始游戏
      await engine.key("ENTER", 1_000);
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("save-select");
          // rgb(72, 120, 8)
          expect(screen.pixel(58, 67)).toEqual([72, 120, 8]);
          // rgb(64, 68, 64)
          expect(screen.pixel(39, 21)).toEqual([64, 68, 64]);
          // rgb(104, 76, 0)
          expect(screen.pixel(145, 13)).toEqual([104, 76, 0]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
    {
      // 选择默认存档，进入任务选择界面
      await engine.key("ENTER", 1_000);
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("job-select-1");
          // rgb(104, 76, 0)
          expect(screen.pixel(144, 8)).toEqual([104, 76, 0]);
          // rgb(0, 0, 0)
          expect(screen.pixel(49, 16)).toEqual([0, 0, 0]);
          // rgb(72, 120, 8)
          expect(screen.pixel(57, 70)).toEqual([72, 120, 8]);
          // rgb(0, 0, 0)
          expect(screen.pixel(71, 113)).toEqual([0, 0, 0]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
    {
      // 选择任务，进入训练选择界面
      await engine.key("ENTER", 1_000);
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("training-select");
          // rgb(104, 76, 0)
          expect(screen.pixel(145, 13)).toEqual([104, 76, 0]);
          // rgb(64, 68, 64)
          expect(screen.pixel(39, 21)).toEqual([64, 68, 64]);
          expect(
            countColor(screen, [152, 152, 152], { x: 49, y: 89, width: 160, height: 30 }),
          ).toBeGreaterThan(4_400);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
    {
      // 选择任务，进入付费界面
      await engine.key("ENTER", 1_000);
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("game-pay");
          // rgb(104, 104, 224)
          expect(screen.pixel(94, 80)).toEqual([104, 104, 224]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
    {
      // 立即返回选择任务界面
      await engine.key("RIGHT_SOFT", 1_000);
      await engine.delay(2_000)
      await vi.waitFor(
        async () => {
          // 取消计费会恢复进入计费前的训练选择页，不会退回上一级任务选择页。
          const screen = await engine!.screen("training-select-return");
          // rgb(104, 76, 0)
          expect(screen.pixel(145, 13)).toEqual([104, 76, 0]);
          // rgb(64, 68, 64)
          expect(screen.pixel(39, 21)).toEqual([64, 68, 64]);
          expect(
            countColor(screen, [152, 152, 152], { x: 49, y: 89, width: 160, height: 30 }),
          ).toBeGreaterThan(4_400);
          // 不应该出现左右箭头
          // rgb(248, 156, 0)
          expect(screen.pixel(227, 153)).not.toEqual([248, 156, 0]);
          // rgb(248, 168, 0)
          expect(screen.pixel(10, 155)).not.toEqual([248, 168, 0]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
  });
});
