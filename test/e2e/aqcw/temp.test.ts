import { execFile, spawn, type ChildProcessByStdio } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { Readable } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SkyEngineE2e, SkyEngineWorkspace } from "../engine-e2e.js";

type PayServerProcess = ChildProcessByStdio<null, Readable, Readable>;

interface PayServer {
  process: PayServerProcess;
  port: number;
  tempDir: string;
}

const execFileAsync = promisify(execFile);

async function startPayServer(): Promise<PayServer> {
  const tempDir = await mkdtemp(path.join(tmpdir(), "skymobi-pay-server-"));
  // Go emits a PE executable on Windows; use its real filename so Node can spawn it
  // when this opt-in temp test is selected explicitly.
  const binary = path.join(
    tempDir,
    process.platform === "win32" ? "skymobi-pay-server.exe" : "skymobi-pay-server",
  );
  const source = path.resolve("tools/pay-server/skymobi-pay-server.go");
  let child: PayServerProcess | undefined;

  try {
    await execFileAsync("go", ["build", "-o", binary, source]);
    child = spawn(binary, [], {
      env: { ...process.env, PORT: "0" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const port = await waitForPayServer(child);
    child.stdout.resume();
    child.stderr.resume();
    return { process: child, port, tempDir };
  } catch (error) {
    await terminatePayServer(child);
    await rm(tempDir, { recursive: true, force: true });
    throw error;
  }
}

function waitForPayServer(child: PayServerProcess): Promise<number> {
  return new Promise((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => fail(new Error(`pay-server readiness timed out: ${stderr}`)), 30_000);

    const cleanup = () => {
      clearTimeout(timer);
      child.stdout.off("data", onStdout);
      child.stderr.off("data", onStderr);
      child.off("error", fail);
      child.off("exit", onExit);
    };
    const fail = (error: Error) => {
      cleanup();
      reject(error);
    };
    const onStdout = (chunk: Buffer) => {
      stdout += chunk.toString();
      const match = /listening on .*:(\d+)/.exec(stdout);
      if (match) {
        cleanup();
        resolve(Number(match[1]));
      }
    };
    const onStderr = (chunk: Buffer) => {
      stderr += chunk.toString();
    };
    const onExit = (code: number | null, signal: NodeJS.Signals | null) => {
      fail(new Error(`pay-server exited before readiness: code=${code} signal=${signal}: ${stderr}`));
    };

    child.stdout.on("data", onStdout);
    child.stderr.on("data", onStderr);
    child.once("error", fail);
    child.once("exit", onExit);
  });
}

async function terminatePayServer(child: PayServerProcess | undefined): Promise<void> {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit").then(() => true);
  child.kill("SIGTERM");
  if (!(await Promise.race([exited, delay(2_000, false)]))) {
    child.kill("SIGKILL");
    await exited;
  }
}

async function stopPayServer(server: PayServer | undefined): Promise<void> {
  if (!server) return;
  try {
    await terminatePayServer(server.process);
  } finally {
    await rm(server.tempDir, { recursive: true, force: true });
  }
}

describe("aqcw", () => {
  let engine: SkyEngineE2e | undefined;
  let ws: SkyEngineWorkspace | undefined;
  let payServer: PayServer | undefined;

  afterEach(async () => {
    await engine?.close();
    engine = undefined;
    await stopPayServer(payServer);
    payServer = undefined;
    await ws?.dispose();
    ws = undefined;
  });

  it("付费", async () => {
    // 每个用例使用独立的 mythroad 数据副本,避免并发执行时互相覆盖插件/缓存/存档。
    ws = await SkyEngineWorkspace.create();
    // 测试拥有独立服务进程和系统分配端口，避免依赖外部进程或固定端口。
    payServer = await startPayServer();
    // 删除后，继续游戏会进入下载浏览器插件界面。
    engine = await SkyEngineE2e.start("test/fixtures/aqcw_1014.mrp", {
      workDir: ws.dir,
      // 显式端口让 DNS 路由令牌把插件写死的 6009 连接转到本地付费服务。
      dnsMap: `rop.skymobiapp.com->127.0.0.1:${payServer.port}`,
    });

    await vi.waitFor(
      async () => {
        const screen = await engine!.screen("main");
        // rgb(240, 240, 240)
        expect(screen.pixel(155, 21)).toEqual([240, 240, 240]);
      },
      {
        timeout: 10_000,
        interval: 1_000,
      },
    );

    // 是否开启音乐？-> 否
    await engine.key("ENTER", 1_000);
    await engine.delay(1_000);

    {
      await engine.key("ENTER", 1_000);
      await vi.waitFor(
        async () => {
          const screen = await engine!.screen("pay");
          // 同时匹配成功提示的首行和末行，避免把仍在等待网络响应的页面判成成功。
          expect(screen.pixel(14, 29)).toEqual([248, 252, 248]);
          expect(screen.pixel(12, 61)).toEqual([248, 252, 248]);
        },
        {
          timeout: 10_000,
          interval: 1_000,
        },
      );
    }
  });
});
