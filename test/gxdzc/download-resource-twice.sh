#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
# 133,295,1000为进入下载界面触发点击事件
# 228,309,500为返回的触发点击事件
export VMRP_AUTO_CLICKS="0,0,500;228,309,500;158,293,1000;133,295,1000;228,309,500;133,295,1000"
build/skyengine mythroad/gxdzc.mrp