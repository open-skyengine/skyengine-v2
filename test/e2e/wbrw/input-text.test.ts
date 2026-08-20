import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";
import fs from "fs";
import { readFile } from "fs/promises";

describe("wbrw 输入文字", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("输入文字", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    fs.rmSync(ws.path('mythroad/brw'), { recursive: true, force: true })

    engine = await SkyEngineE2e.start("test/fixtures/wbrw.mrp", { workDir: ws.dir });
    {
      await engine.delay(5000);
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
      await engine.setClipboard("http://wap.baidu.com");
      // 打开输入界面
      // editCreate只切换宿主编辑状态，不提交位图，不能等待无关的后台重绘。
      await engine.key('ENTER', { holdMs: 120, waitForDraw: false });
      const beforePaste = await engine.screen("before-paste");
      await engine.pasteShortcut();
      const afterPaste = await engine.screen("after-paste");
      const stderr = await readFile(engine.stderrPath, "utf8");

      expect(stderr).toContain("editGetText(): 'http://wap.baidu.com'");
      expect(afterPaste.diffPixelCount(beforePaste, { x: 0, y: 0, width: 240, height: 32 })).toBeGreaterThan(500);
      expect(afterPaste.pixel(99, 17)).toEqual([0, 0, 0]);
      // rgb(248, 252, 248)
      expect(afterPaste.pixel(98, 17)).toEqual([248, 252, 248]);
    }
  });
});
