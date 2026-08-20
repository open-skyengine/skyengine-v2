#!/bin/bash
# 复现 gghjt 付费后进入游戏场景的渲染路径；配合 VMRP_PPM/窄口径诊断环境变量
# 查看场景背景花屏日志，输入节奏与 e2e game-start 流程保持一致。
export VMRP_AUTO_CLICKS="0,0,5000;227,308,2000;227,308,2000;227,308,2000;227,308,2000;227,308,4000;160,290,3000;121,291,1000;-3,0,3000;-4,0,1000;-4,0,8000"
if [ -n "$VMRP_GDB" ]; then
    exec gdb --batch -ex run -ex 'thread apply all bt' --args build/skyengine mythroad/gghjt.mrp
fi
build/skyengine mythroad/gghjt.mrp
