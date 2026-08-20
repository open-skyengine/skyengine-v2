import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage } from "../engine-e2e.js";

function expectMenuSoftkeyLabels(screen: PpmImage): void {
  let hasOkLabel = false;
  let hasBackLabel = false;
  for (let y = 299; y < 315; y++) {
    for (let x = 4; x < 44; x++) {
      if (screen.pixel(x, y).toString() === "0,252,0") hasOkLabel = true;
    }
    for (let x = screen.width - 44; x < screen.width - 4; x++) {
      if (screen.pixel(x, y).toString() === "0,252,0") hasBackLabel = true;
    }
  }
  expect(hasOkLabel).toBe(true);
  expect(hasBackLabel).toBe(true);
}

async function expectNoMainMenuTransition(
  engine: SkyEngineE2e,
  firstDraw: number,
  name: string
): Promise<void> {
  const lastDraw = await engine.drawCount();
  expect(lastDraw).toBeGreaterThan(firstDraw);
  for (let draw = firstDraw + 1; draw <= lastDraw; draw++) {
    const frame = await engine.screenDraw(draw, `${name}-${draw}`);
    // 主菜单标题在该点为 rgb(72, 144, 248)，平台层切换期间不得露出这一帧。
    expect(frame.pixel(40, 13)).not.toEqual([72, 144, 248]);
  }
}

describe("white", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("设置", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });

    await vi.waitFor(async () => { 
        const screen = await engine!.screen("menu")
        // rgb(72, 144, 248)
        expect(screen.pixel(40, 13)).toEqual([72, 144, 248]);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
    await engine.key('DOWN', 1_000);
    await engine.delay(500);
    await engine.key('DOWN', 1_000);
    await engine.delay(500);
    await engine.key('DOWN', 1_000);
    
    await vi.waitFor(async () => { 
        const screen = await engine!.screen("menu-setting-1")
        // rgb(0, 144, 192)
        expect(screen.pixel(165, 221)).toEqual([0, 144, 192]);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
    await engine.key('ENTER', 1_000);

    await vi.waitFor(async () => { 
        const screen = await engine!.screen("setting-1")
        // rgb(0, 0, 248)
        expect(screen.pixel(144, 50)).toEqual([0, 0, 248]);
        expectMenuSoftkeyLabels(screen);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
    const childMenuFirstDraw = await engine.drawCount();
    await engine.key('ENTER', 1_000);
    await expectNoMainMenuTransition(engine, childMenuFirstDraw, "setting-child-transition");

    await vi.waitFor(async () => { 
        const screen = await engine!.screen("setting-1-1")
        // rgb(0, 0, 248)
        expect(screen.pixel(144, 50)).toEqual([0, 0, 248]);
        expectMenuSoftkeyLabels(screen);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
    // 确定选定项
    const savedDialogFirstDraw = await engine.drawCount();
    await engine.key('ENTER', 1_000);
    await expectNoMainMenuTransition(engine, savedDialogFirstDraw, "setting-saved-transition");
    // 保存后 white 会创建 MR_DIALOG_OK 的“设置成功”平台提示；确认并退出
    // 恢复出来的父菜单后，再验证底层 guest 菜单能继续接收方向键。
    await vi.waitFor(async () => {
        const screen = await engine!.screen("setting-saved")
        expect(screen.uniqueColorCount()).toBe(2);
        expect(screen.pixel(0, 294)).toEqual([0, 252, 0]);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
    const parentMenuFirstDraw = await engine.drawCount();
    await engine.key('LEFT_SOFT', 1_000);
    await expectNoMainMenuTransition(engine, parentMenuFirstDraw, "setting-parent-transition");
    // 成功提示关闭后，white 会重新 show 仍在事件栈中的父“游戏设置”菜单。
    await vi.waitFor(async () => {
        const screen = await engine!.screen("setting-parent")
        expect(screen.pixel(144, 50)).toEqual([0, 0, 248]);
        expectMenuSoftkeyLabels(screen);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
    await engine.key('RIGHT_SOFT', 1_000);
    await vi.waitFor(async () => { 
        const screen = await engine!.screen("menu-setting-2")
        // rgb(0, 144, 192)
        expect(screen.pixel(165, 221)).toEqual([0, 144, 192]);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
    await engine.key('UP', 1_000);
    await engine.delay(500);
    await engine.key('UP', 1_000);
    await engine.delay(500);
    await engine.key('UP', 1_000);
    await vi.waitFor(async () => { 
        const screen = await engine!.screen("menu-first")
        // rgb(72, 144, 248)
        expect(screen.pixel(40, 13)).toEqual([72, 144, 248]);
    }, {
        timeout: 30_000,
        interval: 1_000
    });
  }, 240_000);

  it("设置菜单支持触摸", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });

    await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "touch-menu",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    await engine.key("DOWN", 1_000);
    await engine.key("DOWN", 1_000);
    await engine.key("DOWN", 1_000);
    await engine.waitForPixel(165, 221, [0, 144, 192], {
      name: "touch-menu-setting",
      timeoutMs: 30_000,
      intervalMs: 1_000,
    });
    await engine.key("ENTER", 1_000);

    let parent!: PpmImage;
    await vi.waitFor(async () => {
      parent = await engine!.screen("touch-setting-parent");
      expect(parent.pixel(144, 50)).toEqual([0, 0, 248]);
      expectMenuSoftkeyLabels(parent);
    }, {
      timeout: 30_000,
      interval: 1_000,
    });

    /* 先把焦点移到第二项，再点第一项；子菜单必须由触点命中的 index 0 打开，
     * 不能只是把任意点击等价成当前焦点的 ENTER。 */
    await engine.key("DOWN", 1_000);
    const itemFirstDraw = await engine.drawCount();
    await engine.click(144, 50, 2_000);
    let child!: PpmImage;
    await vi.waitFor(async () => {
      child = await engine!.screen("touch-setting-child");
      expect(child.pixel(144, 50)).toEqual([0, 0, 248]);
      expect(child.diffPixelCount(parent)).toBeGreaterThan(0);
      expectMenuSoftkeyLabels(child);
    }, {
      timeout: 30_000,
      interval: 1_000,
    });
    await expectNoMainMenuTransition(engine, itemFirstDraw, "touch-item-transition");

    /* 软键栏左右半区与可见的“确定/返回”一致；返回恢复父菜单，确定再打开
     * 同一个子菜单。PPM 全帧比较同时覆盖触摸回调后的 handle 生命周期。 */
    const backFirstDraw = await engine.drawCount();
    await engine.click(220, 306, 2_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("touch-setting-parent-restored");
      expect(screen.diffPixelCount(parent)).toBe(0);
    }, {
      timeout: 30_000,
      interval: 1_000,
    });
    await expectNoMainMenuTransition(engine, backFirstDraw, "touch-back-transition");

    const okFirstDraw = await engine.drawCount();
    await engine.click(20, 306, 2_000);
    await vi.waitFor(async () => {
      const screen = await engine!.screen("touch-setting-child-reopened");
      expect(screen.diffPixelCount(child)).toBe(0);
    }, {
      timeout: 30_000,
      interval: 1_000,
    });
    await expectNoMainMenuTransition(engine, okFirstDraw, "touch-ok-transition");
  }, 240_000);
});
