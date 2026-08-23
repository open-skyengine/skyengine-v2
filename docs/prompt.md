/goal 根据方案`docs/design.md`以及项目代码，实现功能让`pnpm vitest run test/e2e/geyaxz/boot-to-home.test.ts`能够通过。
禁止参考外部代码
同时以下测试用例也要通过：
pnpm vitest run
白名单机制，不需要全量
下载服务器使用：159.75.119.124
磁盘映射：
c -> 工作区，其它 -> 工作区/disk/x|y|z
mythroad路径：c:/mythroad

/goal 根据方案`docs/design.md`以及项目代码，目前工作区的修改使用了硬编码判断让`pnpm vitest run test/e2e/geyaxz/boot-to-home.test.ts`能够通过；
现在进行重写，改成通用逻辑。
禁止参考外部代码
同时以下测试用例也要通过：
pnpm vitest run
白名单机制，不需要全量