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

/goal 工作区使用硬编码解决游戏启动失败的问题，修正这个规范错误
./skyengine mythroad/gahhx_v1005.mrp 
skyengine: MR fault: @start.mr: line-defined 0, instruction 39/61: ARM fault: unmapped guest access at 0x0000a5c8 (4 bytes) while executing module 0 at PC 0x2002c73c (insn=001092e5, cpsr=0x40000000, r0=0x00200000, r1=0x41f00000, r2=0x0000a5c8, r3=0x83e00001, r4=0x2003ecdd, r5=0x00000000, r6=0x00000000, r7=0x00000000, r8=0x00000000, r9=0x2002f8ac, r10=0x00000000, r11=0x00000000, r12=0x040200c8, sp=0x20001e38, lr=0x2002830b)

1. 禁止参考外部代码，禁止硬编码；
2. 编写测试用例；
3. 同时以下测试用例也要通过：
pnpm vitest run
白名单机制，不需要全量