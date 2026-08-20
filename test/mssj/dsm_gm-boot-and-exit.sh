#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
export VMRP_AUTO_CLICKS="0,0,1000;66,114,500;66,114,1000;81,56,3000;223,308,1000;71,255,1000;122,252,1000"
build/skyengine