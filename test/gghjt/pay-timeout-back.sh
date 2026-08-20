#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
export VMRP_AUTO_CLICKS="0,0,5000;227,308,2000;227,308,2000;227,308,2000;227,308,2000;227,308,4000;160,290,3000;121,291,1000;121,291,60000;227,308,2000"
if [ -n "$VMRP_GDB" ]; then
    exec gdb --batch -ex run -ex 'thread apply all bt' --args build/skyengine mythroad/gghjt.mrp
fi
build/skyengine mythroad/gghjt.mrp