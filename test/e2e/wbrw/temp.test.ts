import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";
import { readFile } from "fs/promises";

describe("wbrw 输入文字", () => {
  const targetURL = process.env.VMRP_WBRW_TEST_URL ?? "http://task.jysafe.cn/so1/?m=wodemo2&k=2009&type=all";
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("task.jysafe.cn/so1", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/brw'), { recursive: true, force: true })

    engine = await SkyEngineE2e.start("test/fixtures/wbrw.mrp", { workDir: ws.dir });
    {
      await engine.delay(2_000);
      const boot = await engine.screen("home");
      // rgb(248, 252, 248)
      expect(boot.pixel(76, 252)).toEqual([248, 252, 248]);
      // rgb(64, 144, 208)
      expect(boot.pixel(57, 306)).toEqual([64, 144, 208]);
    }
    {
      // 移动光标到输入框
      await engine.key('UP', { timeoutMs: 1_000, holdMs: 120 });
      // 进入输入框界面
      await engine.key('ENTER', { timeoutMs: 1_000, holdMs: 120 });
      // 模拟真实操作：用户先复制文本，再打开系统编辑器。
      await engine.setClipboard(targetURL);
      // 打开输入界面
      // editCreate只切换宿主编辑状态，不提交位图，不能等待无关的后台重绘。
      await engine.key('ENTER', { holdMs: 120, waitForDraw: false });
      const beforePaste = await engine.screen("before-paste");
      await engine.pasteShortcut();
      const afterPaste = await engine.screen("after-paste");
      await engine.delay(200);
      const stderr = await readFile(engine.stderrPath, "utf8");

      expect(stderr).toContain(`editGetText(): '${targetURL}'`);
      expect(afterPaste.diffPixelCount(beforePaste, { x: 0, y: 0, width: 240, height: 32 })).toBeGreaterThan(500);
      await engine.delay(1_000);
      // 回车访问
      await engine.key('ENTER', { timeoutMs: 1_000, holdMs: 120 });
      await engine.delay(20_000);
      if (process.env.VMRP_WBRW_REDRAW_AFTER_IMAGE === "1") {
        await engine.key('DOWN', { timeoutMs: 1_000, holdMs: 120 });
        await engine.delay(1_000);
      }
      const loaded = await engine.screen("loaded");
      // The loaded page must replace the address editor and contain the
      // decoded logo plus the quantized section and native form surfaces.
      expect(loaded.diffPixelCount(afterPaste)).toBeGreaterThan(30_000);
      // These source-specific blue and red pixels prove WBRW rendered the
      // bitmap logo rather than its grey alt caption. GIF palette selection
      // can choose adjacent RGB565 blues, so assert the channel family here.
      const logoBlue = loaded.pixel(50, 72);
      expect(logoBlue[0]).toBeLessThanOrEqual(80);
      expect(logoBlue[1]).toBeGreaterThanOrEqual(120);
      expect(logoBlue[1]).toBeLessThanOrEqual(180);
      expect(logoBlue[2]).toBeGreaterThanOrEqual(200);
      expect(loaded.pixel(42, 68)).toEqual([248, 0, 0]);
      // WBRW ignores GIF alpha; proxy adaptation must matte the transparent
      // source canvas instead of exposing its black transparent palette RGB.
      expect(loaded.pixel(100, 43)).toEqual([248, 252, 248]);
      // The final glyph must remain visible near the fitted image's right edge;
      // an unscaled 273px source would extend beyond this 240px framebuffer.
      expect(loaded.pixel(216, 53)).toEqual([152, 152, 152]);
      const sectionGreen = [208, 252, 136] as const;
      expect(loaded.pixel(10, 100)).toEqual(sectionGreen);
      expect(loaded.pixel(40, 192)).toEqual([240, 244, 240]);
      expect(loaded.pixel(100, 192)).toEqual([240, 244, 240]);
      expect(loaded.pixel(180, 192)).toEqual([224, 232, 240]);
      // Both pagination paragraphs belong to the same CSS background container.
      // Sample both edges throughout their former gap so a body-white separator
      // cannot reappear between the previous/next row and the jump form.
      for (let y = 228; y <= 242; y++) {
        expect(loaded.pixel(10, y)).toEqual(sectionGreen);
        expect(loaded.pixel(220, y)).toEqual(sectionGreen);
      }
      for (let x = 5; x <= 230; x++) {
        expect(loaded.pixel(x, 234)).toEqual(sectionGreen);
      }
      // Retain coverage of the quantized footer background after the fitted
      // logo reflows the document upward.
      expect(loaded.pixel(20, 275)).toEqual([248, 252, 176]);
    }
  });
  it("mrp.gddhy.net", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/brw'), { recursive: true, force: true })

    engine = await SkyEngineE2e.start("test/fixtures/wbrw.mrp", { workDir: ws.dir });
    {
      await engine.delay(2_000);
      const boot = await engine.screen("home");
      // rgb(248, 252, 248)
      expect(boot.pixel(76, 252)).toEqual([248, 252, 248]);
      // rgb(64, 144, 208)
      expect(boot.pixel(57, 306)).toEqual([64, 144, 208]);
    }
    {
      // 移动光标到输入框
      await engine.key('UP', { timeoutMs: 1_000, holdMs: 120 });
      // 进入输入框界面
      await engine.key('ENTER', { timeoutMs: 1_000, holdMs: 120 });
      // 模拟真实操作：用户先复制文本，再打开系统编辑器。
      await engine.setClipboard('https://mrp.gddhy.net/');
      // 打开输入界面
      // editCreate只切换宿主编辑状态，不提交位图，不能等待无关的后台重绘。
      await engine.key('ENTER', { holdMs: 120, waitForDraw: false });
      const beforePaste = await engine.screen("before-paste");
      await engine.pasteShortcut();
      const afterPaste = await engine.screen("after-paste");
      await engine.delay(200);
      const stderr = await readFile(engine.stderrPath, "utf8");

      expect(stderr).toContain(`editGetText(): 'https://mrp.gddhy.net/'`);
      expect(afterPaste.diffPixelCount(beforePaste, { x: 0, y: 0, width: 240, height: 32 })).toBeGreaterThan(500);
      await engine.delay(1_000);
      // 回车访问
      await engine.key('ENTER', { timeoutMs: 1_000, holdMs: 120 });
      await engine.delay(10_000);
      if (process.env.VMRP_WBRW_REDRAW_AFTER_IMAGE === "1") {
        await engine.key('DOWN', { timeoutMs: 1_000, holdMs: 120 });
        await engine.delay(1_000);
      }
      const loaded = await engine.screen("loaded");
      // 保存图片视觉检测
      expect(loaded.diffPixelCount(afterPaste)).toBeGreaterThan(30_000);
    }
  });
});
