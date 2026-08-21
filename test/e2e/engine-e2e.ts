import { spawn, type ChildProcessByStdio } from "node:child_process";
import type { Readable } from "node:stream";
import { finished } from "node:stream/promises";
import { createWriteStream, cpSync } from "node:fs";
import { copyFile, mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export type Rgb = readonly [number, number, number];

export interface SkyEngineE2eOptions {
  bin?: string;
  workDir?: string;
  timeoutMs?: number;
  /** 域名重映射(--dns-map),用于把依赖网络的用例约束到本地测试端点。 */
  dnsMap?: string;
  /** 屏幕分辨率(--screen WxH),如 "480x320"。默认由 SkyEngine 决定(240x320)。 */
  screen?: `${number}x${number}`;
  /** 应用可见内存(--memory),档位 1M/2M/4M/6M/8M/16M。默认由 SkyEngine 决定(1M)。 */
  memory?: "1M" | "2M" | "4M" | "6M" | "8M" | "16M";
  /** 应用可见设备日期；"host" 显式使用宿主墙钟日期。 */
  deviceDate?: `${number}-${number}-${number}` | "host";
  /** 每次绘图后更新 defaultScreenPath，不向 SDL 事件队列注入 SCREEN。 */
  captureLatestFrame?: boolean;
}

export interface SkyEngineKeyOptions {
  timeoutMs?: number;
  holdMs?: number;
  waitForDraw?: boolean;
}

export interface SkyEnginePasteOptions {
  timeoutMs?: number;
  waitForDraw?: boolean;
}

export interface PpmImage {
  width: number;
  height: number;
  pixel(x: number, y: number): Rgb;
  uniqueColorCount(): number;
  diffPixelCount(other: PpmImage, rect?: { x: number; y: number; width: number; height: number }): number;
}

type KeyName =
  | "ENTER" | "SELECT"
  | "ESC" | "ESCAPE" | "POWER"
  | "SOFTLEFT" | "LEFT_SOFT"
  | "SOFTRIGHT" | "RIGHT_SOFT"
  | "UP" | "DOWN" | "LEFT" | "RIGHT"
  | "SEND"
  | "STAR" | "*" 
  | "POUND" | "HASH" | "#"
  | `${number}`

export class SkyEngineE2e {
  readonly tmpDir: string;
  readonly socketPath: string;
  readonly stdoutPath: string;
  readonly stderrPath: string;
  readonly defaultScreenPath: string;

  private readonly bin: string;
  private readonly workDir: string;
  private readonly timeoutMs: number;
  private readonly dnsMap?: string;
  /** 命名避免与 screen() 方法冲突:实例字段会遮蔽原型方法。 */
  private readonly screenSize?: string;
  private readonly memorySize?: string;
  private readonly deviceDate?: string;
  private readonly captureLatestFrame: boolean;
  private process?: ChildProcessByStdio<null, Readable, Readable>;
  private spawnError?: Error;
  private stdoutLog?: ReturnType<typeof createWriteStream>;
  private stderrLog?: ReturnType<typeof createWriteStream>;

  private constructor(tmpDir: string, options: SkyEngineE2eOptions = {}) {
    this.tmpDir = tmpDir;
    this.socketPath = controlEndpoint(tmpDir);
    this.stdoutPath = path.join(tmpDir, "stdout.log");
    this.stderrPath = path.join(tmpDir, "stderr.log");
    this.defaultScreenPath = path.join(tmpDir, "screen.ppm");
    this.bin = resolveSkyEngineBin(options.bin);
    this.workDir = options.workDir ?? process.env.SKYENGINE_WORK_DIR ?? ".";
    this.timeoutMs = options.timeoutMs ?? Number(process.env.VMRP_TIMEOUT_MS ?? 30_000);
    this.dnsMap = options.dnsMap;
    this.screenSize = options.screen;
    this.memorySize = options.memory;
    this.deviceDate = options.deviceDate;
    this.captureLatestFrame = options.captureLatestFrame ?? false;
  }

  static async start(mrpPath: string, options: SkyEngineE2eOptions = {}): Promise<SkyEngineE2e> {
    const tmpDir = await mkdtemp(path.join(tmpdir(), "skyengine-e2e-"));
    const engine = new SkyEngineE2e(tmpDir, options);
    try {
      await engine.spawn(await engine.prepareMrp(mrpPath));
    } catch (error) {
      // Windows keeps log files locked until their streams close; clean failed starts here
      // so an ENOENT or IPC startup error does not leak a process or temp directory.
      await engine.close();
      throw error;
    }
    // 同时输出 CI 收集的截图目录和模拟器工作目录，便于从用例日志定位产物。
    console.info(`[skyengine-e2e] artifact-dir: ${tmpDir}; work-dir: ${path.resolve(engine.workDir)}`);
    return engine;
  }

  /**
   * 确保 MRP 文件位于 work-dir 之下,否则复制一份进去再启动。
   *
   * ARM EXT 执行器把包名 alias 解析为"相对 cwd 的最短可打开路径"
   * (src/arm_ext_executor.c arm_ext_init_pack_names),且部分 EXT helper
   * 只有固定 32 字节的包名缓冲区。当 MRP 位于 work-dir 之外时 alias 退化
   * 为截断的绝对路径,包自身无法被重新打开,wbrw 等应用会误判包异常并
   * 触发联网版本上报,改变启动流程导致像素断言失败。仓库根作为 work-dir
   * 时 fixtures 本就在 cwd 下,此函数不做任何复制,保持原有行为。
   */
  private async prepareMrp(mrpPath: string): Promise<string> {
    const workDir = path.resolve(this.workDir);
    const absMrp = path.resolve(mrpPath);
    const relative = path.relative(workDir, absMrp);
    if (relative && relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative)) {
      return mrpPath;
    }
    const dest = path.join(workDir, path.basename(absMrp));
    await copyFile(absMrp, dest);
    return dest;
  }

  async close(): Promise<void> {
    await this.stop();
    if (process.env.VMRP_E2E_KEEP_TMP !== "1") {
      await rm(this.tmpDir, { recursive: true, force: true });
    }
  }

  async stop(): Promise<void> {
    const proc = this.process;
    if (proc && proc.exitCode === null) {
      try {
        await this.command("QUIT", 2_000);
      } catch {
        proc.kill("SIGTERM");
      }
      if (!(await this.waitForExit(5_000))) {
        proc.kill("SIGKILL");
        await this.waitForExit(2_000);
      }
    }
    /* Some tests inspect stdout immediately after stop(); wait for destination
     * streams, not only the child close event, so Windows file buffers are final. */
    await Promise.all([
      this.stdoutLog ? finished(this.stdoutLog) : Promise.resolve(),
      this.stderrLog ? finished(this.stderrLog) : Promise.resolve()
    ]);
  }

  async drawCount(): Promise<number> {
    const response = await this.command("DRAW_COUNT");
    const match = /^OK draw_count (\d+)$/.exec(response);
    if (!match) throw new Error(`Unexpected DRAW_COUNT response: ${response}`);
    return Number(match[1]);
  }

  async click(x: number, y: number, timeoutMs = 2_000): Promise<void> {
    const previous = await this.drawCount();
    await this.command(`CLICK ${x} ${y}`);
    await this.waitDrawAfter(previous, timeoutMs);
  }

  async delay(ms: number): Promise<void> {
    await sleep(ms);
  }

  /**
   * Supported key names:
   * ENTER/SELECT, ESC/ESCAPE/POWER, SOFTLEFT/LEFT_SOFT, SOFTRIGHT/RIGHT_SOFT,
   * UP, DOWN, LEFT, RIGHT, SEND, STAR/*, POUND/HASH/#, digits 0-9, letters A-Z.
   *
   * holdMs: 显式物理按住时长(毫秒),覆盖 VMRP_E2E_KEY_HOLD_MS；显式时长可
   * 表达重复键或长按菜单。KEY 响应携带运行时线程取走 KEYDOWN 时的 draw
   * baseline，避免把排队期间无关的 timer 帧误当作按键结果。
   */
  async key(
    name: KeyName,
    optionsOrTimeout: SkyEngineKeyOptions | number = 2_000,
    legacyHoldMs?: number
  ): Promise<void> {
    const options = typeof optionsOrTimeout === "number"
      ? { timeoutMs: optionsOrTimeout, holdMs: legacyHoldMs, waitForDraw: true }
      : { timeoutMs: 2_000, waitForDraw: true, ...optionsOrTimeout };
    const previous = options.waitForDraw ? await this.drawCount() : undefined;
    const response = await this.command(options.holdMs != null ? `KEY ${name} ${options.holdMs}` : `KEY ${name}`);
    const accepted = /^OK key draw_count (\d+)$/.exec(response);
    const drawBaseline = accepted ? Number(accepted[1]) : previous;
    // editCreate and other valid state-only actions do not submit a bitmap.  Let
    // callers express that contract instead of accepting an unrelated timer draw.
    // A key may also complete by intentionally terminating the runtime; the server
    // reports that terminal state explicitly because no later draw can exist.
    if (drawBaseline != null && !response.endsWith(" exited")) {
      await this.waitDrawAfter(drawBaseline, options.timeoutMs);
    }
  }

  async paste(text: string, timeoutMs = 5_000): Promise<void> {
    await this.setClipboard(text);
    await this.pasteShortcut(timeoutMs);
  }

  async setClipboard(text: string): Promise<void> {
    await this.command(`SET_CLIPBOARD ${text}`);
  }

  async pasteShortcut(optionsOrTimeout: SkyEnginePasteOptions | number = 5_000): Promise<void> {
    const options = typeof optionsOrTimeout === "number"
      ? { timeoutMs: optionsOrTimeout, waitForDraw: true }
      : { timeoutMs: 5_000, waitForDraw: true, ...optionsOrTimeout };
    const previous = options.waitForDraw ? await this.drawCount() : undefined;
    await this.command("PASTE_SHORTCUT");
    if (previous != null) await this.waitDrawAfter(previous, options.timeoutMs);
  }

  async screen(name = "screen"): Promise<PpmImage> {
    const ppmPath = path.join(this.tmpDir, `${name}.ppm`);
    await this.command(`SCREEN ${ppmPath}`);
    return readPpm(ppmPath);
  }

  async screenDraw(drawCount: number, name = `draw-${drawCount}`): Promise<PpmImage> {
    const ppmPath = path.join(this.tmpDir, `${name}.ppm`);
    await this.command(`SCREEN_DRAW ${drawCount} ${ppmPath}`);
    return readPpm(ppmPath);
  }

  /**
   * 轮询截屏直到 (x,y) 处像素等于 expected,返回命中时的截图供后续断言。
   * 适用于等待时长不确定的画面(如游戏结束结算),替代固定 delay + 单次断言。
   * 超时抛错并附带最后一次实际像素值,便于修正断言点。
   */
  async waitForPixel(
    x: number,
    y: number,
    expected: Rgb,
    options: { name?: string; timeoutMs?: number; intervalMs?: number } = {}
  ): Promise<PpmImage> {
    const { name = "wait-pixel", timeoutMs = 60_000, intervalMs = 1_000 } = options;
    const deadline = Date.now() + timeoutMs;
    let last: Rgb | undefined;
    for (;;) {
      const screen = await this.screen(name);
      last = screen.pixel(x, y);
      if (last[0] === expected[0] && last[1] === expected[1] && last[2] === expected[2]) {
        return screen;
      }
      if (Date.now() + intervalMs > deadline) break;
      await sleep(intervalMs);
    }
    throw new Error(
      `Pixel (${x},${y}) did not become rgb(${expected.join(",")}) within ${timeoutMs}ms; last was rgb(${last?.join(",")})`
    );
  }

  async command(command: string, timeoutMs = this.timeoutMs): Promise<string> {
    return new Promise((resolve, reject) => {
      const socket = createConnection(this.socketPath);
      let response = "";
      socket.setEncoding("utf8");
      socket.setTimeout(timeoutMs);
      socket.once("connect", () => socket.write(`${command}\n`));
      socket.on("data", chunk => {
        response += chunk;
        if (response.includes("\n")) {
          const line = response.slice(0, response.indexOf("\n")).trim();
          /* Windows named-pipe disconnect may surface as EPIPE after the complete
           * response was read. The newline is the protocol completion boundary. */
          socket.destroy();
          if (line.startsWith("OK")) resolve(line);
          else reject(new Error(line || `Empty response to ${command}`));
        }
      });
      socket.once("timeout", () => {
        socket.destroy();
        reject(new Error(`Timed out waiting for response to ${command}`));
      });
      socket.once("error", reject);
      socket.once("end", () => {
        const line = response.trim();
        if (line.startsWith("OK")) resolve(line);
        else reject(new Error(line || `Empty response to ${command}`));
      });
    });
  }

  private async spawn(mrpPath: string): Promise<void> {
    const args = ["--work-dir", this.workDir];
    if (this.dnsMap !== undefined) args.push("--dns-map", this.dnsMap);
    if (this.screenSize) args.push("--screen", this.screenSize);
    if (this.memorySize) args.push("--memory", this.memorySize);
    if (this.deviceDate) args.push("--device-date", this.deviceDate);
    args.push(mrpPath);
    this.process = spawn(this.bin, args, {
      env: {
        ...process.env,
        SDL_VIDEODRIVER: process.env.SDL_VIDEODRIVER ?? "dummy",
        SDL_AUDIODRIVER: process.env.SDL_AUDIODRIVER ?? "dummy",
        SKYENGINE_E2E_SOCKET: this.socketPath,
        SKYENGINE_PPM_PATH: this.defaultScreenPath,
        ...(this.captureLatestFrame ? { SKYENGINE_PPM: "1" } : {})
      },
      stdio: ["ignore", "pipe", "pipe"]
    });
    this.process.once("error", error => {
      this.spawnError = error;
    });

    this.stdoutLog = createWriteStream(this.stdoutPath);
    this.stderrLog = createWriteStream(this.stderrPath);
    this.process.stdout.pipe(this.stdoutLog);
    this.process.stderr.pipe(this.stderrLog);

    await this.waitForSocket();
  }

  private async waitForSocket(): Promise<void> {
    const start = Date.now();
    const proc = this.process;
    if (!proc) throw new Error("SkyEngine process was not created");
    while (Date.now() - start < this.timeoutMs) {
      if (this.spawnError) {
        throw new Error(`Failed to start SkyEngine (${this.bin}): ${this.spawnError.message}`);
      }
      if (proc.exitCode !== null || proc.signalCode !== null) {
        throw new Error(
          `SkyEngine exited before E2E endpoint was ready: code=${proc.exitCode}, signal=${proc.signalCode}`
        );
      }
      try {
        if (process.platform === "win32") {
          // Windows named pipes are not filesystem nodes. A protocol round-trip proves
          // that both CreateNamedPipe and the command loop are ready.
          await this.command("DRAW_COUNT", 250);
          return;
        }
        const info = await stat(this.socketPath);
        if (info.isSocket()) return;
      } catch {
        // The local IPC endpoint is created asynchronously after SDL and VM startup.
      }
      await sleep(25);
    }
    throw new Error(`E2E endpoint was not ready after ${this.timeoutMs}ms`);
  }

  private async waitDrawAfter(previous: number, timeoutMs: number): Promise<void> {
    await this.command(`WAIT_DRAW ${previous} ${timeoutMs}`);
  }

  async waitForExit(timeoutMs: number): Promise<boolean> {
    const proc = this.process;
    if (!proc || proc.exitCode !== null || proc.signalCode !== null || this.spawnError) return true;
    return new Promise(resolve => {
      const done = (exited: boolean) => {
        clearTimeout(timer);
        proc.off("close", onClose);
        proc.off("error", onError);
        resolve(exited);
      };
      const onClose = () => done(true);
      const onError = () => done(true);
      const timer = setTimeout(() => done(false), timeoutMs);
      proc.once("close", onClose);
      proc.once("error", onError);
      // Close/error may have raced with listener registration.
      if (proc.exitCode !== null || proc.signalCode !== null || this.spawnError) done(true);
    });
  }
}

/** CMake's documented native output for each host platform. */
export function defaultSkyEngineBin(): string {
  return process.platform === "win32"
    ? "build/Release/skyengine.exe"
    : "build/skyengine";
}

/**
 * Keep direct spawn callers on the same executable selection as SkyEngineE2e.
 * CI flattens packaged binaries into build/, so VMRP_BIN must win over the
 * multi-config Windows default even when no SkyEngineE2e instance is created.
 */
export function resolveSkyEngineBin(bin?: string): string {
  return bin ?? process.env.VMRP_BIN ?? defaultSkyEngineBin();
}

function controlEndpoint(tmpDir: string): string {
  if (process.platform === "win32") {
    // Keep the pipe name ASCII and independent of a profile/TEMP path containing spaces.
    return `\\\\.\\pipe\\skyengine-e2e-${process.pid}-${path.basename(tmpDir)}`;
  }
  return path.join(tmpDir, "skyengine-e2e.sock");
}

/**
 * 每个测试用例独立的 mythroad 数据副本。
 *
 * 测试并发执行时,所有用例共享同一个 mythroad 目录会互相覆盖插件/缓存/存档,
 * 导致数据竞争。SkyEngineWorkspace 在临时目录中复制一份模板,把该目录作为
 * SkyEngine 的 --work-dir,实现文件级隔离。
 *
 * 生命周期须覆盖整个 it()(而非单个 VmrpE2e 实例):opglqa/font.test.ts
 * 会在同一用例内重启第二个 SkyEngine 实例并验证第一次启动落盘的文件被复用。
 */
export class SkyEngineWorkspace {
  /** 传给 VmrpE2e.start 的 workDir。 */
  readonly dir: string;

  private constructor(dir: string) {
    this.dir = dir;
  }

  static async create(): Promise<SkyEngineWorkspace> {
    const tmpDir = await mkdtemp(path.join(tmpdir(), "skyengine-ws-"));
    // 用 import.meta.url 定位模板源,不依赖 worker 的 cwd。
    const src = fileURLToPath(new URL("../fixtures/mythroad", import.meta.url));
    // preserveTimestamps: true —— 模拟器/MRP 应用会依据文件时间戳判断
    // “是否需要重新联网校验”(如 wbrw 启动时的 simpleDownload 上报);
    // 不保留 mtime 会触发额外的真实网络请求,拖慢启动导致像素断言失败。
    cpSync(src, path.join(tmpDir, "mythroad"), {
      recursive: true,
      dereference: true,
      preserveTimestamps: true
    });
    return new SkyEngineWorkspace(tmpDir);
  }

  /** 把工作区内的相对路径(如 "mythroad/plugins/netpay.mrp")转成绝对路径。 */
  path(rel: string): string {
    return path.join(this.dir, rel);
  }

  async dispose(): Promise<void> {
    if (process.env.VMRP_E2E_KEEP_TMP !== "1") {
      await rm(this.dir, { recursive: true, force: true });
    }
  }
}

export async function readPpm(filePath: string): Promise<PpmImage> {
  const buffer = await readFile(filePath);
  let offset = 0;

  const token = (): string => {
    while (offset < buffer.length && isSpace(buffer[offset])) offset++;
    if (buffer[offset] === 35) {
      while (offset < buffer.length && buffer[offset] !== 10) offset++;
      return token();
    }
    const start = offset;
    while (offset < buffer.length && !isSpace(buffer[offset])) offset++;
    return buffer.subarray(start, offset).toString("ascii");
  };

  const magic = token();
  if (magic !== "P6") throw new Error(`Unsupported PPM magic: ${magic}`);
  const width = Number(token());
  const height = Number(token());
  const maxValue = Number(token());
  if (maxValue !== 255) throw new Error(`Unsupported PPM max value: ${maxValue}`);
  if (isSpace(buffer[offset])) offset++;

  const pixels = buffer.subarray(offset);
  const expectedBytes = width * height * 3;
  if (pixels.length < expectedBytes) {
    throw new Error(`PPM pixel data is truncated: got ${pixels.length}, expected ${expectedBytes}`);
  }
  return {
    width,
    height,
    pixel(x: number, y: number): Rgb {
      if (x < 0 || y < 0 || x >= width || y >= height) {
        throw new Error(`Pixel out of bounds: ${x},${y} for ${width}x${height}`);
      }
      const index = (y * width + x) * 3;
      return [pixels[index], pixels[index + 1], pixels[index + 2]];
    },
    uniqueColorCount(): number {
      const colors = new Set<number>();
      for (let i = 0; i < expectedBytes; i += 3) {
        colors.add((pixels[i] << 16) | (pixels[i + 1] << 8) | pixels[i + 2]);
      }
      return colors.size;
    },
    diffPixelCount(other: PpmImage, rect = { x: 0, y: 0, width, height }): number {
      if (other.width !== width || other.height !== height) {
        throw new Error(`PPM dimensions differ: ${width}x${height} vs ${other.width}x${other.height}`);
      }
      let count = 0;
      const x0 = Math.max(0, rect.x);
      const y0 = Math.max(0, rect.y);
      const x1 = Math.min(width, rect.x + rect.width);
      const y1 = Math.min(height, rect.y + rect.height);
      for (let y = y0; y < y1; y++) {
        for (let x = x0; x < x1; x++) {
          const index = (y * width + x) * 3;
          const rhs = other.pixel(x, y);
          if (pixels[index] !== rhs[0] || pixels[index + 1] !== rhs[1] || pixels[index + 2] !== rhs[2]) {
            count++;
          }
        }
      }
      return count;
    }
  };
}

function isSpace(byte: number | undefined): boolean {
  return byte === 9 || byte === 10 || byte === 11 || byte === 12 || byte === 13 || byte === 32;
}

function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}
