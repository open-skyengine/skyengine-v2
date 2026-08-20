import { copyFile } from "node:fs/promises";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage } from "../engine-e2e.js";

const PROMPT_RECT = { x: 76, y: 294, width: 92, height: 18 };
const INTERACTION_TEXT_RECT = { x: 50, y: 155, width: 145, height: 45 };

function matchingPixelCount(
  image: PpmImage,
  rect: { x: number; y: number; width: number; height: number },
  predicate: (red: number, green: number, blue: number) => boolean,
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      if (predicate(...image.pixel(x, y))) count++;
    }
  }
  return count;
}

function promptGlyphPixelCount(image: PpmImage): number {
  return matchingPixelCount(image, PROMPT_RECT, (red, green, blue) => red + green + blue < 384);
}

function interactionGlyphPixelCount(image: PpmImage): number {
  return matchingPixelCount(image, INTERACTION_TEXT_RECT, (red, green, blue) => red + green + blue > 600);
}

describe("gtlbd", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("初始提示和交互文字在定时器推进后仍然显示", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();

    const mrp = ws.path("mythroad/gtlbd.mrp");
    await copyFile("test/fixtures/gtlbd.mrp", mrp);
    // 模拟设备已安装的公共 netpay 版本；使用固定 fixture，不依赖共享 build 状态。
    await copyFile(
      "test/fixtures/plugins/netpay-original.mrp",
      ws.path("mythroad/plugins/netpay.mrp"),
    );
    // 相对设备路径参与 EXT 的包别名生成；在隔离 cwd 中启动可复现
    // mythroad/gtlbd.mrp 语义，同时不读取或写入共享 build/mythroad。
    engine = await SkyEngineE2e.start("test/fixtures/gtlbd.mrp", {
      captureLatestFrame: true,
      workDir: ws.dir,
    });

    // 启动已在进入事件循环前完成首帧绘制；只截一次，避免控制请求改变 timer 竞态。
    const initialPrompt = await engine.screen("initial-prompt");
    expect(promptGlyphPixelCount(initialPrompt)).toBeGreaterThan(200);
    expect(promptGlyphPixelCount(initialPrompt)).toBeLessThan(500);

    await engine.delay(5_000);
    const retainedPrompt = await engine.screen("retained-prompt");

    await engine.key("ENTER", 5_000);
    const interactionText = await engine.screen("interaction-text");

    await engine.delay(5_000);
    const retainedInteractionText = await engine.screen("retained-interaction-text");

    // 先采集完整时间线再断言，失败运行也会保留交互后的诊断帧。
    expect(promptGlyphPixelCount(retainedPrompt)).toBeGreaterThan(200);
    expect(promptGlyphPixelCount(retainedPrompt)).toBeLessThan(500);
    expect(initialPrompt.diffPixelCount(retainedPrompt, PROMPT_RECT)).toBe(0);
    expect(interactionGlyphPixelCount(retainedInteractionText)).toBeGreaterThan(800);
    expect(interactionGlyphPixelCount(retainedInteractionText)).toBeLessThan(2_000);
    expect(interactionGlyphPixelCount(interactionText)).toBeGreaterThan(800);
    expect(interactionGlyphPixelCount(interactionText)).toBeLessThan(2_000);
    expect(interactionText.diffPixelCount(retainedInteractionText, INTERACTION_TEXT_RECT)).toBe(0);
  });
});
