#!/bin/bash
# 删除插件，应用会请求下载
rm -f mythroad/plugins/netpay.mrp
export VMRP_AUTO_CLICKS="0,0,3000;227,308,2000;227,308,2000;227,308,2000;227,308,2000;160,290,1000;121,291,1000;5,308,1000;227,308,1000;5,308,1000"
if [ -n "$VMRP_GDB" ]; then
    exec gdb --batch -ex run -ex 'thread apply all bt' --args build/skyengine mythroad/gghjt.mrp
fi
build/skyengine mythroad/gghjt.mrp
