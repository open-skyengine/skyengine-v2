import { afterEach, describe, expect, it } from "vitest";
import {
  SkyEngineE2e,
  SkyEngineWorkspace,
  type PpmImage,
  type Rgb,
} from "../engine-e2e.js";

function hasColor(
  screen: PpmImage,
  expected: Rgb,
  rect: { x: number; y: number; width: number; height: number },
): boolean {
  for (let y = rect.y; y < rect.y + rect.height; y++) {
    for (let x = rect.x; x < rect.x + rect.width; x++) {
      if (screen.pixel(x, y).toString() === expected.toString()) return true;
    }
  }
  return false;
}

function legalMoves(screen: PpmImage): Array<{ x: number; y: number }> {
  const moves = [];
  for (let row = 0; row < 8; row++) {
    for (let column = 0; column < 8; column++) {
      let blackPixels = 0;
      for (let y = 40 + row * 30; y < 62 + row * 30; y++) {
        for (let x = 4 + column * 30; x < 26 + column * 30; x++) {
          if (screen.pixel(x, y).toString() === "0,0,0") blackPixels++;
        }
      }
      // 普通 X 标记为 51 个黑点，带焦点圈的 X 为 118 个；棋子至少为 209 个。
      if (blackPixels >= 40 && blackPixels <= 150) {
        moves.push({ x: column * 30 + 15, y: row * 30 + 51 });
      }
    }
  }
  return moves;
}

async function startGame(engine: SkyEngineE2e): Promise<PpmImage> {
  await engine.waitForPixel(40, 13, [72, 144, 248], {
    name: "gameplay-main-menu",
    timeoutMs: 30_000,
    intervalMs: 1_000,
  });
  await engine.click(120, 103, 1_000);
  await engine.waitForPixel(0, 294, [0, 252, 0], {
    name: "gameplay-registration",
    timeoutMs: 30_000,
    intervalMs: 250,
  });
  await engine.click(20, 306, 1_000);
  await engine.waitForPixel(144, 50, [0, 0, 248], {
    name: "gameplay-new-game-menu",
    timeoutMs: 30_000,
    intervalMs: 250,
  });
  await engine.click(20, 306, 1_000);
  return engine.waitForPixel(120, 10, [16, 192, 240], {
    name: "gameplay-initial-board",
    timeoutMs: 30_000,
    intervalMs: 250,
  });
}

describe("white 对局", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("可以完成一回合并操作局内选项", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });
    const initialBoard = await startGame(engine);

    // 初始局面的左上合法点，位于第 3 行第 4 列。
    await engine.click(105, 111, 5_000);
    const playedBoard = await engine.waitForScreen(
      screen => screen.diffPixelCount(
        initialBoard,
        { x: 0, y: 36, width: 240, height: 240 },
      ) > 200 && !hasColor(
        screen,
        [248, 252, 248],
        { x: 96, y: 0, width: 96, height: 27 },
      ),
      { name: "gameplay-after-computer-move", timeoutMs: 30_000, intervalMs: 250 },
    );

    await engine.click(200, 302, 5_000);
    const options = await engine.screen("gameplay-options");
    expect(options.diffPixelCount(playedBoard)).toBeGreaterThan(1_000);

    await engine.click(120, 46, 5_000);
    await engine.waitForScreen(
      screen => screen.diffPixelCount(playedBoard) === 0,
      { name: "gameplay-continued-board", timeoutMs: 30_000, intervalMs: 100 },
    );

    await engine.click(200, 302, 5_000);
    await engine.click(120, 70, 5_000);
    const undoneBoard = await engine.waitForScreen(
      screen => screen.pixel(120, 10).toString() === "16,192,240"
        && screen.diffPixelCount(playedBoard) > 200,
      { name: "gameplay-undone-board", timeoutMs: 30_000, intervalMs: 100 },
    );
    expect(undoneBoard.diffPixelCount(playedBoard)).toBeGreaterThan(200);

    await engine.click(200, 302, 5_000);
    await engine.click(120, 70, 5_000);
    const exitResult = await engine.waitForScreen(
      screen => screen.pixel(40, 13).toString() === "72,144,248",
      { name: "gameplay-exit-result", timeoutMs: 30_000, intervalMs: 100 },
    );
    expect(exitResult.diffPixelCount(undoneBoard)).toBeGreaterThan(1_000);
  }, 120_000);

  it("可以连续落子直到一局结束", async () => {
    ws = await SkyEngineWorkspace.create();
    engine = await SkyEngineE2e.start("test/fixtures/white.mrp", { workDir: ws.dir });
    let board = await startGame(engine);
    let completedMoves = 0;
    let result: PpmImage | undefined;

    for (let turn = 0; turn < 60; turn++) {
      const move = legalMoves(board)[0];
      expect(move, `第 ${turn + 1} 回合没有可识别的合法落点`).toBeDefined();
      await engine.click(move!.x, move!.y, 5_000);
      const next = await engine.waitForScreen(
        screen => screen.pixel(120, 10).toString() !== "16,192,240"
          || (legalMoves(screen).length > 0
            && screen.diffPixelCount(board, { x: 0, y: 36, width: 240, height: 240 }) > 200),
        { name: `full-game-turn-${turn + 1}`, timeoutMs: 30_000, intervalMs: 100 },
      );
      completedMoves++;
      if (next.pixel(120, 10).toString() !== "16,192,240") {
        result = next;
        break;
      }
      board = next;
    }

    expect(completedMoves).toBeGreaterThan(10);
    expect(result, "60 次玩家落子后仍未出现结算画面").toBeDefined();
    const resultScreen = await engine.screen("full-game-result");
    expect(resultScreen.uniqueColorCount()).toBe(2);
    await engine.key("RIGHT_SOFT", 5_000);
    await engine.waitForPixel(40, 13, [72, 144, 248], {
      name: "full-game-returned-menu",
      timeoutMs: 30_000,
      intervalMs: 250,
    });
  }, 300_000);
});
