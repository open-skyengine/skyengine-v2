import { readFile } from "node:fs/promises";
import { afterEach, describe, expect, it } from "vitest";
import { type PpmImage, SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

const ISTORE_OFFLINE_DNS_MAP = [
  "rop.skymobiapp.com->127.0.0.1",
  "spd.skymobiapp.com->127.0.0.1",
  "wap.skmeg.com->127.0.0.1",
].join(";");

describe("istore 进入主界面", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  async function openHome() {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    const activeEngine = await SkyEngineE2e.start("test/fixtures/sky_istore.mrp", {
      workDir: ws.dir,
      memory: "2M",
      dnsMap: ISTORE_OFFLINE_DNS_MAP,
    });
    engine = activeEngine;

    await activeEngine.waitForScreen(screen =>
      screen.pixel(123, 78).toString() === "112,152,208"
      && screen.pixel(143, 162).toString() === "216,220,216"
      && screen.pixel(121, 205).toString() === "168,208,80", {
        name: "network-error",
        timeoutMs: 30_000,
        intervalMs: 1_000,
      });

    await activeEngine.click(120, 215, 5_000);
    const home = await activeEngine.waitForScreen(screen =>
      screen.pixel(23, 291).toString() === "144,144,144"
      && screen.pixel(70, 291).toString() === "72,76,72"
      && screen.pixel(120, 70).toString() === "176,156,104"
      && screen.pixel(200, 140).toString() === "56,92,0", {
        name: "home",
        timeoutMs: 30_000,
        intervalMs: 500,
      });
    return { engine: activeEngine, home };
  }

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("确认网络错误后可以打开分类页", async () => {
    const { engine, home } = await openHome();

    await engine.delay(1_000);
    await engine.click(72, 305, 5_000);
    const category = await engine.waitForScreen(screen =>
      screen.pixel(70, 291).toString() === "32,32,32"
      && screen.pixel(23, 291).toString() === "72,76,72"
      && screen.pixel(116, 128).toString() === "216,220,216", {
        name: "category",
        timeoutMs: 90_000,
        intervalMs: 500,
      });

    const nav = { x: 0, y: 288, width: 96, height: 32 };
    expect(home.diffPixelCount(category, nav)).toBeGreaterThan(2_000);
    expect(await engine.waitForExit(1_000)).toBe(false);

    await engine.stop();
    const stderr = await readFile(engine.stderrPath, "utf8");
    expect(stderr).not.toMatch(
      /ARM fault|ABI error|unmapped|no memory|guest heap exhausted|panicked at/i,
    );
  }, 150_000);

  it("确认网络错误后可以打开搜索页", async () => {
    const { engine, home } = await openHome();

    await engine.delay(1_000);
    await engine.click(168, 305, 5_000);
    const search = await engine.waitForScreen(screen =>
      screen.pixel(166, 291).toString() === "32,32,32"
      && screen.pixel(23, 291).toString() === "72,76,72"
      && screen.pixel(200, 38).toString() === "112,152,208"
      && screen.pixel(10, 61).toString() === "216,220,216"
      && screen.pixel(10, 84).toString() === "224,228,224", {
        name: "search",
        timeoutMs: 30_000,
        intervalMs: 500,
      });

    const content = { x: 0, y: 25, width: 240, height: 263 };
    expect(home.diffPixelCount(search, content)).toBeGreaterThan(20_000);
    expect(await engine.waitForExit(1_000)).toBe(false);

    await engine.stop();
    const stderr = await readFile(engine.stderrPath, "utf8");
    expect(stderr).not.toMatch(
      /ARM fault|ABI error|unmapped|no memory|guest heap exhausted|panicked at/i,
    );
  }, 90_000);

  it("可以重复进入并返回幻灯片详情页", async () => {
    const { engine } = await openHome();

    for (let attempt = 1; attempt <= 4; attempt += 1) {
      await engine.click(120, 70, 30_000);
      await engine.waitForScreen(screen =>
        screen.pixel(10, 38).toString() === "96,136,200"
        && screen.pixel(120, 150).toString() === "80,88,80"
        && screen.pixel(200, 305).toString() === "64,64,64", {
          name: `slideshow-detail-${attempt}`,
          timeoutMs: 30_000,
          intervalMs: 250,
        });
      expect(await engine.waitForExit(500)).toBe(false);

      await engine.click(216, 38, 30_000);
      await engine.waitForScreen(screen =>
        screen.pixel(23, 291).toString() === "144,144,144"
        && screen.pixel(70, 291).toString() === "72,76,72"
        && screen.pixel(120, 70).toString() === "176,156,104"
        && screen.pixel(200, 140).toString() === "56,92,0", {
          name: `slideshow-home-${attempt}`,
          timeoutMs: 30_000,
          intervalMs: 250,
        });
      expect(await engine.waitForExit(500)).toBe(false);
    }

    await engine.stop();
    const stderr = await readFile(engine.stderrPath, "utf8");
    expect(stderr).not.toMatch(
      /ARM fault|ABI error|unmapped|no memory|guest heap exhausted|panicked at/i,
    );
  }, 180_000);

  it("从详情页返回后可以随机切换底部菜单", async () => {
    const { engine, home } = await openHome();

    await engine.click(120, 70, 30_000);
    await engine.waitForScreen(screen =>
      screen.pixel(10, 38).toString() === "96,136,200"
      && screen.pixel(120, 150).toString() === "80,88,80"
      && screen.pixel(200, 305).toString() === "64,64,64", {
        name: "switch-detail",
        timeoutMs: 30_000,
        intervalMs: 250,
      });

    await engine.click(220, 30, 30_000);
    await engine.waitForScreen(screen =>
      screen.pixel(23, 291).toString() === "144,144,144"
      && screen.pixel(70, 291).toString() === "72,76,72"
      && screen.pixel(120, 70).toString() === "176,156,104"
      && screen.pixel(200, 140).toString() === "56,92,0", {
        name: "returned-home",
        timeoutMs: 30_000,
        intervalMs: 250,
      });

    const menuPages = [
      { name: "home", click: [24, 309], marker: [23, "144,144,144"] },
      { name: "category", click: [72, 306], marker: [70, "32,32,32"] },
      { name: "ranking", click: [120, 309], marker: [118, "32,32,32"] },
      { name: "search", click: [168, 305], marker: [166, "32,32,32"] },
      { name: "manage", click: [216, 304], marker: [214, "32,32,32"] },
    ] as const;
    const markerXs = menuPages.map(page => page.marker[0]);
    const matchesMenu = (screen: PpmImage, activeIndex: number) => markerXs.every(
      (markerX, index) => screen.pixel(markerX, 291).toString()
        === (index === activeIndex ? menuPages[index].marker[1] : "72,76,72"),
    );
    const isLoading = (screen: PpmImage) =>
      screen.pixel(10, 140).toString() === "72,72,72"
      && screen.pixel(120, 150).toString() === "24,24,24";
    const isStablePage = (screen: PpmImage, menuIndex: number) => {
      if (!matchesMenu(screen, menuIndex)) return false;
      if (menuIndex === 0) {
        return screen.pixel(120, 70).toString() === "176,156,104"
          && screen.pixel(200, 140).toString() === "56,92,0";
      }
      if (menuIndex === 3) {
        return screen.pixel(200, 38).toString() === "112,152,208"
          && screen.pixel(10, 61).toString() === "216,220,216"
          && screen.pixel(10, 84).toString() === "224,228,224";
      }
      if (menuIndex === 4) {
        return screen.pixel(10, 38).toString() === "96,136,200"
          && screen.pixel(10, 84).toString() === "216,220,216"
          && screen.pixel(10, 140).toString() === "96,136,200"
          && screen.pixel(10, 200).toString() === "216,220,216";
      }
      return !isLoading(screen)
        && screen.pixel(10, 140).toString() === "216,216,216";
    };
    const waitForMenuPage = (menuIndex: number, name: string) =>
      engine.waitForScreen(screen => isStablePage(screen, menuIndex), {
        name,
        timeoutMs: 90_000,
        intervalMs: 100,
      });
    const selectMenu = async (menuIndex: number, name: string) => {
      const [x, y] = menuPages[menuIndex].click;
      let lastError: unknown;
      for (let attempt = 1; attempt <= 3; attempt += 1) {
        await engine.click(x, y, 10_000);
        try {
          await engine.waitForScreen(screen => matchesMenu(screen, menuIndex), {
            name: `${name}-selected`,
            timeoutMs: 2_000,
            intervalMs: 50,
          });
          return;
        } catch (error) {
          lastError = error;
        }
      }
      throw lastError;
    };
    const openMenu = async (menuIndex: number, name: string) => {
      await selectMenu(menuIndex, name);
      if (menuIndex === 1 || menuIndex === 2) {
        await engine.waitForScreen(screen => matchesMenu(screen, menuIndex) && isLoading(screen), {
          name: `${name}-loading`,
          timeoutMs: 10_000,
          intervalMs: 50,
        });
      }
      return waitForMenuPage(menuIndex, name);
    };

    const homeScreenshot = await waitForMenuPage(0, "bottom-menu-1-home");
    expect(homeScreenshot.uniqueColorCount()).toBeGreaterThan(100);

    for (const [menuIndex, page] of menuPages.entries()) {
      if (menuIndex === 0) continue;
      const screenshot = await openMenu(
        menuIndex,
        `bottom-menu-${menuIndex + 1}-${page.name}`,
      );
      expect(screenshot.uniqueColorCount()).toBeGreaterThan(100);
      expect(home.diffPixelCount(screenshot, { x: 0, y: 288, width: 240, height: 32 }))
        .toBeGreaterThan(2_000);
    }

    await openMenu(0, "stress-home");

    const reportedClicks = [
      [63, 309],
      [110, 306],
      [160, 307],
      [168, 303],
      [215, 304],
      [117, 306],
    ] as const;
    for (const [clickIndex, [x, y]] of reportedClicks.entries()) {
      await engine.click(x, y, 10_000);
      expect(await engine.waitForExit(250), `reported click ${clickIndex + 1} (${x}, ${y})`)
        .toBe(false);
    }

    const stressOrder = [3, 0, 4, 1, 2, 4, 2, 3, 1, 0] as const;
    for (let round = 0; round < 6; round += 1) {
      for (const [clickIndex, menuIndex] of stressOrder.entries()) {
        const [x, y] = menuPages[menuIndex].click;
        await engine.click(x, y, 10_000);
        const sequenceIndex = reportedClicks.length + round * stressOrder.length + clickIndex + 1;
        expect(await engine.waitForExit(250), `stress click ${sequenceIndex} (${x}, ${y})`)
          .toBe(false);
      }
      await selectMenu(0, `stress-round-${round + 1}-home`);
      await waitForMenuPage(0, `stress-round-${round + 1}-home`);
    }

    await engine.stop();
    const stderr = await readFile(engine.stderrPath, "utf8");
    expect(stderr).not.toMatch(
      /ARM fault|ABI error|unmapped|no memory|guest heap exhausted|panicked at/i,
    );
  }, 480_000);
});
