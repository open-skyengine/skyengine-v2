#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
export VMRP_AUTO_CLICKS="14,308,2000;44,285,2000;14,308,2000"
build/skyengine mythroad/mpc.mrp