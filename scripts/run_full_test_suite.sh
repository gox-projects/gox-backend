#!/usr/bin/env bash
# GX 渲染后端全量测试统一入口。
#
# 覆盖三层（自底向上）：
#   1. gx_gpu 独立 crate
#      - CPU feature：契约测试（数据模型/C ABI/null 安全）+ 像素级光栅测试
#      - GPU feature：契约测试 + wgpu 管线单测（无需窗口，纯逻辑）
#   2. gxbackend_rust（Rust 后端 crate）
#      - 场景协议零拷贝解析、GRX2/GRX3 畸形帧防御、渲染内核单测
#   3. rust/go 桥接层（Go CGO）
#      - GRX3 编码字节布局、parseChangeID 边界、畸形字节喂 Rust 不 panic
#
# 可选第四层：GUI E2E（需已编译的 gox-idea 二进制 + X11/截图工具）。
#    RUN_GUI_E2E=1 GOX_IDEA_BIN=/path/to/gox-idea ./run_full_test_suite.sh
#
# 用法：
#   ./run_full_test_suite.sh                  # 核心三层
#   RUN_GUI_E2E=1 ./run_full_test_suite.sh    # 含 GUI E2E（二进制走默认路径）
#
# 退出码：所有套件全绿返回 0，任一失败返回 1。
# 设计：每个套件独立执行并汇总，不用 set -e，确保一次跑完全部、暴露所有失败。
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GPU_DIR="$ROOT/gpu"
RUST_CRATE="$ROOT/rust/rust"
GO_DIR="$ROOT/rust/go"

PASS=0
FAIL=0
FAILED=""

run_suite() {
  local name="$1"; shift
  echo ""
  echo "================ $name ================"
  if "$@"; then
    echo ">>> $name : OK"
    PASS=$((PASS+1))
  else
    echo ">>> $name : FAILED"
    FAIL=$((FAIL+1))
    FAILED="$FAILED $name"
  fi
}

# ---- 套件 1：gx_gpu CPU feature（含像素级光栅）----
suite_gpu_cpu() {
  cd "$GPU_DIR" || return 1
  cargo test --no-default-features --features cpu 2>&1 | tee /tmp/gx-test-gpu-cpu.log
  # 管道退出码取 cargo（pipefail 手动判定）
  grep -q 'test result: FAILED' /tmp/gx-test-gpu-cpu.log && return 1
  grep -qE 'error\[|error: could not compile' /tmp/gx-test-gpu-cpu.log && return 1
  # 至少要有测试跑过
  grep -qE 'test result: ok' /tmp/gx-test-gpu-cpu.log || return 1
  return 0
}

# ---- 套件 2：gx_gpu GPU feature（wgpu 管线纯逻辑）----
suite_gpu_gpu() {
  cd "$GPU_DIR" || return 1
  cargo test 2>&1 | tee /tmp/gx-test-gpu-gpu.log
  grep -q 'test result: FAILED' /tmp/gx-test-gpu-gpu.log && return 1
  grep -qE 'error\[|error: could not compile' /tmp/gx-test-gpu-gpu.log && return 1
  grep -qE 'test result: ok' /tmp/gx-test-gpu-gpu.log || return 1
  return 0
}

# ---- 套件 3：Rust 后端 crate ----
suite_rust() {
  cd "$RUST_CRATE" || return 1
  cargo test 2>&1 | tee /tmp/gx-test-rust.log
  grep -q 'test result: FAILED' /tmp/gx-test-rust.log && return 1
  grep -qE 'error\[|error: could not compile' /tmp/gx-test-rust.log && return 1
  grep -qE 'test result: ok' /tmp/gx-test-rust.log || return 1
  return 0
}

# ---- 套件 4：Go 桥接层 ----
suite_go() {
  cd "$GO_DIR" || return 1
  # 静态库缺失时给出明确提示而非链接报错。
  if ! ls "$RUST_CRATE"/target/release/libgxbackend_rust.* >/dev/null 2>&1; then
    echo "  [SKIP-FAIL] Rust 静态库未编译，请先 cargo build --release"
    return 1
  fi
  go test ./... -count=1 2>&1 | tee /tmp/gx-test-go.log
  grep -qE '^FAIL|--- FAIL' /tmp/gx-test-go.log && return 1
  grep -qE 'cannot find|undefined reference|build failed' /tmp/gx-test-go.log && return 1
  return 0
}

# ---- 套件 5（可选）：GUI E2E ----
suite_gui_e2e() {
  local e2e_dir="/workspace/gox/cmd/gox-idea/tests"
  if [ ! -d "$e2e_dir" ]; then
    echo "  [SKIP] 未找到 GUI E2E 目录: $e2e_dir"
    return 0
  fi
  for tool in Xvfb xdotool xclip; do
    command -v "$tool" >/dev/null 2>&1 || { echo "  [SKIP] 缺 $tool"; return 0; }
  done
  cd "$e2e_dir" || return 1
  bash run_gui_e2e_all.sh 2>&1 | tee /tmp/gx-test-gui-e2e.log
  grep -qE '失败套件|FAILED' /tmp/gx-test-gui-e2e.log && return 1
  return 0
}

echo "############################################"
echo "# GX 渲染后端全量测试套件"
echo "# 根目录: $ROOT"
echo "############################################"

run_suite "gx_gpu (CPU feature · 契约+像素光栅)" suite_gpu_cpu
run_suite "gx_gpu (GPU feature · wgpu 管线)" suite_gpu_gpu
run_suite "gxbackend_rust (协议+渲染内核)" suite_rust
run_suite "rust/go (CGO 桥接+GRX3 契约)" suite_go

if [ "${RUN_GUI_E2E:-0}" = "1" ]; then
  run_suite "GUI E2E (Xvfb+xdotool 实机交互)" suite_gui_e2e
else
  echo ""
  echo "================ GUI E2E (跳过) ================"
  echo "如需运行：RUN_GUI_E2E=1 GOX_IDEA_BIN=/path/to/gox-idea $0"
fi

echo ""
echo "================ 汇总 ================"
echo "套件通过=$PASS 失败=$FAIL${FAILED:+ ; 失败套件:$FAILED}"
[ "$FAIL" -eq 0 ]
