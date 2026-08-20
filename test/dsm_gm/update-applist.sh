#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
export VMRP_AUTO_CLICKS="0,0,1000;75,172,500;75,172,500;59,58,500;59,58,500"
build/skyengine