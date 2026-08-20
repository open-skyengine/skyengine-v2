#!/usr/bin/env bash
# 复现 gzwdzjs 自动点击流程。使用隔离工作目录以包含完整的 plugins/
# 资源（缺少 netpay.mrp 时游戏走异常路径导致栈损坏崩溃）。
set -e
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WS=$(mktemp -d /tmp/gzwdzjs-repro-XXXXXX)
cp -rL "$ROOT/test/fixtures/mythroad" "$WS/mythroad"
cp "$ROOT/test/fixtures/gzwdzjs.mrp" "$WS/mythroad/"
seq='-2,0,3000;-4,0,1000;-4,0,1000;-4,0,1000;-4,0,1000;-4,0,1000;-4,0,1000;-4,0,15000;-4,0,15000'
timeout 90s env VMRP_AUTO_CLICK_DELAY_MS=5000 VMRP_AUTO_CLICKS="$seq" \
  "$ROOT/build/skyengine" --work-dir "$WS" "$WS/mythroad/gzwdzjs.mrp"
rc=$?
rm -rf "$WS"
exit $rc
