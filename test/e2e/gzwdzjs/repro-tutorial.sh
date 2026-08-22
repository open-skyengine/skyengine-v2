#!/usr/bin/env bash
# gzwdzjs 教程流程手动复现脚本(不经 vitest),对应 game-start.test.ts "游戏教程开始"。
# 用法: repro-tutorial.sh <输出目录> [额外环境变量 KEY=VAL ...]
set -e
OUT="${1:?usage: repro-tutorial.sh <outdir>}"
shift || true
mkdir -p "$OUT"
ROOT="${SKYENGINE_ROOT:-$(cd "$(dirname "$0")/../../.." && pwd)}"
WS=$(mktemp -d /tmp/gzwdzjs-ws-XXXXXX)
cp -rL "$ROOT/test/fixtures/mythroad" "$WS/mythroad"
cp "$ROOT/test/fixtures/gzwdzjs.mrp" "$WS/"
SOCK="$OUT/e2e.sock"

for kv in "$@"; do export "$kv"; done

SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy \
SKYENGINE_E2E_SOCKET="$SOCK" \
"$ROOT/build/skyengine" --work-dir "$WS" --memory 2M "$WS/gzwdzjs.mrp" \
  >"$OUT/stdout.log" 2>"$OUT/stderr.log" &
SKYENGINE_PID=$!

# 环境中没有 socat,用 python3 实现同样的"发送一行命令并读取一行响应"。
cmd() { python3 - "$SOCK" "$1" <<'PYEOF'
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(30)
s.connect(sys.argv[1])
s.sendall((sys.argv[2] + "\n").encode())
buf = b""
while b"\n" not in buf:
    d = s.recv(4096)
    if not d: break
    buf += d
print(buf.decode(errors="replace").strip())
PYEOF
}

for i in $(seq 1 200); do [ -S "$SOCK" ] && break; sleep 0.1; done
sleep 5
cmd "SCREEN $OUT/bgm-select.ppm"
cmd "KEY RIGHT_SOFT"; sleep 3
cmd "SCREEN $OUT/menu.ppm"
# 7 下 ENTER,进入"读取中,请等待"
for i in 1 2 3 4 5 6 7; do cmd "KEY ENTER"; sleep 1; cmd "SCREEN $OUT/enter-$i.ppm"; done
# 等待读取完成
sleep 10
cmd "SCREEN $OUT/after-load.ppm"
# 再 5 下 ENTER
for i in 1 2 3 4 5; do cmd "KEY ENTER"; sleep 1; cmd "SCREEN $OUT/enter2-$i.ppm"; done
cmd "SCREEN $OUT/tutorial.ppm"
cmd "KEY LEFT_SOFT"; sleep 5
cmd "SCREEN $OUT/start.ppm"
cmd "QUIT" || kill $SKYENGINE_PID
wait $SKYENGINE_PID 2>/dev/null || true
rm -rf "$WS"
echo "done: $OUT"
