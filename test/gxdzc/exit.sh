#!/bin/bash
# up和down要间隔500ms；每个事件都显式带 delay，便于复现付费超时路径。
# 123,291,500为退出游戏按钮的点击事件，没有确认框，应当直接退出
export VMRP_AUTO_CLICKS="0,0,500;228,309,500;76,291,500;123,291,500"
build/skyengine mythroad/gxdzc.mrp