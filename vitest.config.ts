import { defineConfig } from "vitest/config";

export default defineConfig(() => {
  const isTargetingTemp = process.argv.some(arg => arg.includes('temp.test.ts'))
  return {
    test: {
      retry: 0,
      include: ["test/e2e/**/*.test.ts"],
      exclude: [
        ...(isTargetingTemp ? [] : ['test/e2e/**/temp.test.ts'])
      ],
      testTimeout: 600_000,
      hookTimeout: 30_000,
      // 测试文件并发执行(vitest 默认),数据隔离由 SkyEngineWorkspace 提供:
      // 每个用例复制独立的 mythroad/ 到临时目录并作为 SkyEngine 的 --work-dir。
      // 每个文件同一时刻只跑一个 SkyEngine(Unicorn ARM 模拟,CPU 密集)。上限 8
      // 个并发文件是为了在 22 核机器上给模拟器和系统留余量,避免 CPU 超售
      // 导致固定 delay + 像素断言的用例出现时序抖动。
      poolOptions: { forks: { maxForks: 8 } }
    }
  }
});
