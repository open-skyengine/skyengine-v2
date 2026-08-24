import { afterEach, describe, expect, it } from "vitest";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import {
  SkyEngineE2e,
  SkyEngineWorkspace,
  type PpmImage,
  type Rgb,
} from "../engine-e2e.js";

const MENU_BACKGROUND = [24, 160, 200] as const satisfies Rgb;
const SELECTED_BACKGROUND = [216, 228, 240] as const satisfies Rgb;

function differingPixels(
  screen: PpmImage,
  rect: { x: number; y: number; width: number; height: number },
  background: Rgb,
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      const pixel = screen.pixel(x, y);
      if (pixel[0] !== background[0] || pixel[1] !== background[1] || pixel[2] !== background[2]) {
        count++;
      }
    }
  }
  return count;
}

function expectSelectedMenuRow(screen: PpmImage, y: number): void {
  // The selection background starts after the icon and extends beyond the text.
  expect(screen.pixel(150, y + 8)).toEqual(SELECTED_BACKGROUND);
  expect(differingPixels(screen, { x: 5, y, width: 16, height: 16 }, MENU_BACKGROUND)).toBeGreaterThan(8);
  expect(differingPixels(screen, { x: 26, y, width: 96, height: 16 }, SELECTED_BACKGROUND)).toBeGreaterThan(16);
}

function isSelectedMenuRow(screen: PpmImage, y: number): boolean {
  const highlight = screen.pixel(150, y + 8);
  return (
    highlight[0] === SELECTED_BACKGROUND[0] &&
    highlight[1] === SELECTED_BACKGROUND[1] &&
    highlight[2] === SELECTED_BACKGROUND[2] &&
    differingPixels(screen, { x: 5, y, width: 16, height: 16 }, MENU_BACKGROUND) > 8 &&
    differingPixels(screen, { x: 26, y, width: 96, height: 16 }, SELECTED_BACKGROUND) > 16
  );
}

function hasFourthMenuRow(screen: PpmImage): boolean {
  return differingPixels(screen, { x: 5, y: 105, width: 118, height: 16 }, MENU_BACKGROUND) > 8;
}

function isKeyBindingScreen(screen: PpmImage): boolean {
  return (
    screen.pixel(150, 53).toString() === MENU_BACKGROUND.toString() &&
    differingPixels(screen, { x: 0, y: 45, width: 120, height: 16 }, MENU_BACKGROUND) > 16
  );
}

function isUpdateMenu(screen: PpmImage): boolean {
  return (
    isSelectedMenuRow(screen, 45) &&
    differingPixels(screen, { x: 5, y: 65, width: 118, height: 16 }, MENU_BACKGROUND) > 8 &&
    differingPixels(screen, { x: 5, y: 85, width: 118, height: 16 }, MENU_BACKGROUND) === 0
  );
}

function isInstalledApplicationMenu(screen: PpmImage): boolean {
  return (
    isSelectedMenuRow(screen, 45) &&
    [65, 85, 105].every(
      y => differingPixels(screen, { x: 5, y, width: 118, height: 16 }, MENU_BACKGROUND) > 8,
    ) &&
    differingPixels(screen, { x: 5, y: 125, width: 118, height: 16 }, MENU_BACKGROUND) === 0
  );
}

function isUpdateDialingScreen(screen: PpmImage): boolean {
  const iconPixels = differingPixels(
    screen,
    { x: 90, y: 120, width: 60, height: 61 },
    MENU_BACKGROUND,
  );
  const progressPixels = differingPixels(
    screen,
    { x: 7, y: 287, width: 226, height: 5 },
    MENU_BACKGROUND,
  );
  return (
    screen.pixel(150, 53).toString() === MENU_BACKGROUND.toString() &&
    iconPixels > 3_000 &&
    progressPixels > 1_000
  );
}

function isUpdatedApplicationList(screen: PpmImage, selectedY = 45): boolean {
  return (
    isSelectedMenuRow(screen, selectedY) &&
    [65, 85, 105, 125, 145, 165, 185].every(
      (y) =>
        differingPixels(screen, { x: 5, y, width: 16, height: 16 }, MENU_BACKGROUND) > 8 &&
        differingPixels(screen, { x: 26, y, width: 150, height: 16 }, MENU_BACKGROUND) > 16,
    )
  );
}

async function mrpFiles(directory: string): Promise<string[]> {
  return (await readdir(directory))
    .filter(name => name.toLowerCase().endsWith(".mrp"))
    .sort();
}

function changedPixels(
  before: PpmImage,
  after: PpmImage,
  rect: { x: number; y: number; width: number; height: number },
): number {
  let count = 0;
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      const first = before.pixel(x, y);
      const second = after.pixel(x, y);
      if (first[0] !== second[0] || first[1] !== second[1] || first[2] !== second[2]) count++;
    }
  }
  return count;
}

describe("dsm_gm", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("向下移动后完整绘制焦点行", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/dsm_gm.mrp", {
      workDir: ws.dir,
      dnsMap:
        "rop.skymobiapp.com->159.75.119.124;" +
        "spd.skymobiapp.com->159.75.119.124;" +
        "proxy.51mrp.com->127.0.0.1;" +
        "proxy2.51mrp.com->127.0.0.1",
    });

    const initial = await engine.waitForScreen(
      (screen) =>
        screen.pixel(150, 53).toString() === SELECTED_BACKGROUND.toString() &&
        differingPixels(screen, { x: 5, y: 45, width: 16, height: 16 }, MENU_BACKGROUND) > 8,
      { name: "initial-menu", timeoutMs: 10_000, intervalMs: 250 },
    );
    expectSelectedMenuRow(initial, 45);

    await engine.key("DOWN", { timeoutMs: 1_000, holdMs: 80 });
    await engine.delay(250);
    const movedOnce = await engine.screen("moved-once-menu");
    expectSelectedMenuRow(movedOnce, 65);

    await engine.key("DOWN", { timeoutMs: 1_000, holdMs: 80 });
    await engine.delay(250);
    const movedTwice = await engine.screen("moved-twice-menu");
    expectSelectedMenuRow(movedTwice, 85);
  });

  it("保存设置", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/dsm_gm.mrp", {
      workDir: ws.dir,
      dnsMap:
        "rop.skymobiapp.com->159.75.119.124;" +
        "spd.skymobiapp.com->159.75.119.124;" +
        "proxy.51mrp.com->127.0.0.1;" +
        "proxy2.51mrp.com->127.0.0.1",
    });

    const initial = await engine.waitForScreen(
      (screen) => isSelectedMenuRow(screen, 45) && hasFourthMenuRow(screen),
      { name: "save-initial-menu", timeoutMs: 10_000, intervalMs: 250 },
    );
    expectSelectedMenuRow(initial, 45);

    await engine.key("DOWN", { timeoutMs: 1_000, holdMs: 80 });
    const selectedSettings = await engine.waitForScreen((screen) => isSelectedMenuRow(screen, 65), {
      name: "save-selected-settings",
      timeoutMs: 1_000,
      intervalMs: 50,
    });
    expectSelectedMenuRow(selectedSettings, 65);

    await engine.key("ENTER", { timeoutMs: 1_000, holdMs: 80 });
    const settings = await engine.waitForScreen(
      (screen) => isSelectedMenuRow(screen, 45) && !hasFourthMenuRow(screen),
      { name: "settings-menu", timeoutMs: 1_000, intervalMs: 50 },
    );
    expectSelectedMenuRow(settings, 45);

    // 右软键保存设置并返回主菜单。
    await engine.key("RIGHT_SOFT", { timeoutMs: 1_000, holdMs: 80 });
    const saved = await engine.waitForScreen(
      (screen) => isSelectedMenuRow(screen, 65) && hasFourthMenuRow(screen),
      { name: "saved-main-menu", timeoutMs: 2_000, intervalMs: 50 },
    );
    expectSelectedMenuRow(saved, 65);
  });

  it("按键绑定", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/dsm_gm.mrp", {
      workDir: ws.dir,
      dnsMap:
        "rop.skymobiapp.com->159.75.119.124;" +
        "spd.skymobiapp.com->159.75.119.124;" +
        "proxy.51mrp.com->127.0.0.1;" +
        "proxy2.51mrp.com->127.0.0.1",
    });

    const initial = await engine.waitForScreen(
      (screen) => isSelectedMenuRow(screen, 45) && hasFourthMenuRow(screen),
      { name: "key-initial-menu", timeoutMs: 10_000, intervalMs: 250 },
    );
    expectSelectedMenuRow(initial, 45);

    await engine.key("DOWN", { timeoutMs: 1_000, holdMs: 80 });
    const selectedSettings = await engine.waitForScreen((screen) => isSelectedMenuRow(screen, 65), {
      name: "key-selected-settings",
      timeoutMs: 1_000,
      intervalMs: 50,
    });
    expectSelectedMenuRow(selectedSettings, 65);

    await engine.key("ENTER", { timeoutMs: 1_000, holdMs: 80 });
    const settings = await engine.waitForScreen(
      (screen) => isSelectedMenuRow(screen, 45) && !hasFourthMenuRow(screen),
      { name: "key-settings-menu", timeoutMs: 1_000, intervalMs: 50 },
    );
    expectSelectedMenuRow(settings, 45);

    await engine.key("UP", { timeoutMs: 1_000, holdMs: 80 });
    const selectedBinding = await engine.waitForScreen((screen) => isSelectedMenuRow(screen, 85), {
      name: "key-selected-binding",
      timeoutMs: 1_000,
      intervalMs: 50,
    });
    expectSelectedMenuRow(selectedBinding, 85);

    await engine.key("ENTER", { timeoutMs: 1_000, holdMs: 80 });
    const bindingUp = await engine.waitForScreen(isKeyBindingScreen, {
      name: "key-binding-up",
      timeoutMs: 1_000,
      intervalMs: 50,
    });

    await engine.key("LEFT_SOFT", { timeoutMs: 1_000, holdMs: 80 });
    const bindingDown = await engine.waitForScreen(
      (screen) =>
        isKeyBindingScreen(screen) &&
        changedPixels(bindingUp, screen, { x: 82, y: 45, width: 22, height: 16 }) > 4,
      { name: "key-binding-down", timeoutMs: 1_000, intervalMs: 50 },
    );
    expect(changedPixels(bindingUp, bindingDown, { x: 82, y: 45, width: 22, height: 16 })).toBeGreaterThan(4);
  });

  it("更新列表后下载并识别第 2 个软件", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/dsm_gm.mrp", {
      workDir: ws.dir,
      dnsMap: "10.0.0.172->159.75.119.124:13230",
    });

    const initial = await engine.waitForScreen(
      (screen) => isSelectedMenuRow(screen, 45) && hasFourthMenuRow(screen),
      { name: "update-initial-menu", timeoutMs: 10_000, intervalMs: 250 },
    );
    expectSelectedMenuRow(initial, 45);

    await engine.key("ENTER", { timeoutMs: 1_000, holdMs: 80 });
    const updateMenu = await engine.waitForScreen(isUpdateMenu, {
      name: "update-menu",
      timeoutMs: 1_000,
      intervalMs: 50,
    });
    expectSelectedMenuRow(updateMenu, 45);
    expect(
      changedPixels(initial, updateMenu, { x: 26, y: 45, width: 96, height: 36 }),
    ).toBeGreaterThan(100);

    await engine.key("ENTER", { timeoutMs: 1_000, holdMs: 80 });
    const dialing = await engine.waitForScreen(isUpdateDialingScreen, {
      name: "update-dialing",
      timeoutMs: 1_000,
      intervalMs: 50,
    });
    expect(isUpdateDialingScreen(dialing)).toBe(true);

    const updated = await engine.waitForScreen(isUpdatedApplicationList, {
      name: "public-update-result",
      timeoutMs: 15_000,
      intervalMs: 100,
    });
    expect(isUpdatedApplicationList(updated)).toBe(true);
    expect(changedPixels(dialing, updated, { x: 0, y: 42, width: 240, height: 164 })).toBeGreaterThan(5_000);

    const mythroadDirectory = path.join(ws.dir, "mythroad");
    const updatedPackage = await readFile(path.join(mythroadDirectory, "applist.mrp"));
    expect(updatedPackage.subarray(0, 4).toString("ascii")).toBe("MRPG");

    await engine.key("DOWN", { timeoutMs: 2_000, holdMs: 80 });
    await engine.waitForScreen(screen => isUpdatedApplicationList(screen, 65), {
      name: "selected-second-application",
      timeoutMs: 2_000,
      intervalMs: 50,
    });

    const packagesBeforeDownload = await mrpFiles(mythroadDirectory);
    await engine.key("ENTER", { timeoutMs: 2_000, holdMs: 80 });
    const downloadMenu = await engine.waitForScreen(isUpdateMenu, {
      name: "download-application-menu",
      timeoutMs: 2_000,
      intervalMs: 50,
    });
    expect(changedPixels(updated, downloadMenu, { x: 0, y: 42, width: 240, height: 164 })).toBeGreaterThan(100);

    await engine.key("ENTER", { timeoutMs: 2_000, holdMs: 80 });
    const installedList = await engine.waitForScreen(screen => isUpdatedApplicationList(screen), {
      name: "installed-application-list",
      timeoutMs: 60_000,
      intervalMs: 100,
    });
    const packagesAfterDownload = await mrpFiles(mythroadDirectory);
    const downloadedPackages = packagesAfterDownload.filter(
      name => !packagesBeforeDownload.includes(name),
    );
    expect(downloadedPackages).toEqual(["gfhcq.mrp"]);
    const installedPackage = await readFile(path.join(mythroadDirectory, downloadedPackages[0]));
    const expectedPackage = await readFile("test/fixtures/gfhcq.mrp");
    expect(installedPackage.equals(expectedPackage)).toBe(true);
    // 已安装应用会被提升到列表首行；前两行文字重排是状态刷新成功的视觉证据。
    expect(changedPixels(updated, installedList, { x: 26, y: 45, width: 150, height: 36 })).toBeGreaterThan(500);

    await engine.key("ENTER", { timeoutMs: 2_000, holdMs: 80 });
    const runMenu = await engine.waitForScreen(isInstalledApplicationMenu, {
      name: "run-application-menu",
      timeoutMs: 2_000,
      intervalMs: 50,
    });
    expect(changedPixels(downloadMenu, runMenu, { x: 26, y: 45, width: 96, height: 16 })).toBeGreaterThan(8);
  });
});
