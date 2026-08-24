import fs from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace, type PpmImage } from "../engine-e2e.js";

describe("optwar 字体与声音", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  const start = async () => {
    ws = await SkyEngineWorkspace.create();
    if (!fs.existsSync(ws.path("mythroad/plugins/netpay.mrp"))) {
      fs.cpSync("test/fixtures/plugins/netpay.mrp", ws.path("mythroad/plugins/netpay.mrp"));
    }
    if (!fs.existsSync(ws.path("mythroad/plugins/advbar.mrp"))) {
      fs.cpSync("test/fixtures/plugins/advbar.mrp", ws.path("mythroad/plugins/advbar.mrp"));
    }
    engine = await SkyEngineE2e.start("test/fixtures/optwar.mrp", { workDir: ws.dir });
  };

  const rowContainsGreen = (screen: PpmImage, y: number, x0: number, x1: number) => {
    for (let x = x0; x <= x1; x++) {
      if (screen.pixel(x, y).toString() === "0,252,0") return true;
    }
    return false;
  };

  it("字库校验百分比使用完整的 ASCII 字模", async () => {
    await start();
    await engine!.delay(1_000);

    const latest = await engine!.drawCount();
    let validation: PpmImage | undefined;
    for (let draw = Math.max(1, latest - 127); draw <= latest; draw++) {
      let frame: PpmImage;
      try {
        frame = await engine!.screenDraw(draw, `font-validation-${draw}`);
      } catch {
        continue;
      }
      if (frame.pixel(78, 142).toString() === "0,252,0") {
        validation = frame;
        break;
      }
    }

    expect(validation, "启动帧历史中应包含字库校验画面").toBeDefined();
    // 旧实现把 8 像素字模按 2 字节行距交给 guest，因此奇数扫描行全空。
    expect(rowContainsGreen(validation!, 169, 100, 140)).toBe(true);
  });

  it("声音菜单接受循环播放并保持运行", async () => {
    await start();
    await engine!.delay(2_000);

    // 是否开启音乐？-> 否
    await engine!.click(227, 301, 1_000);
    await engine!.delay(1_000);

    // 关闭前台广告条，依次切换到“自由选关”和“声音：关/开”。
    await engine!.key("RIGHT_SOFT", 1_000);
    await engine!.delay(500);
    await engine!.key("RIGHT", 1_000);
    await engine!.delay(500);
    await engine!.key("RIGHT", 1_000);
    const soundOff = await engine!.waitForScreen(
      screen =>
        screen.pixel(101, 263).toString() === "0,252,0" &&
        screen.pixel(135, 263).toString() === "0,252,0" &&
        screen.pixel(124, 265).toString() === "0,252,0",
      { name: "sound-off", timeoutMs: 3_000, intervalMs: 100 }
    );

    // optwar 以 -1 请求循环 MIDI；确认后不能触发 ABI fault。
    await engine!.key("ENTER", 3_000);
    const soundOn = await engine!.screen("sound-on");

    expect(soundOn.diffPixelCount(soundOff)).toBeGreaterThan(0);
    expect(await engine!.command("DRAW_COUNT")).toMatch(/^OK draw_count \d+$/);
  });
});
