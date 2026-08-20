#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
export VMRP_AUTO_CLICKS="0,0,500;228,309,500;158,293,1000;133,295,3000;228,309,500"
build/skyengine mythroad/gxdzc.mrp