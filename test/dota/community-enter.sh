#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
export VMRP_AUTO_CLICKS="0,0,3000;223,308,1000;223,308,1000;57,172,500;57,172,500;63,225,500"
build/skyengine mythroad/dota.mrp 