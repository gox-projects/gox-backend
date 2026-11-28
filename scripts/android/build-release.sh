#!/usr/bin/env bash
# 一键构建 GX Playground Android Release APK（gomobile + Rust V2 能力层）。
#
# 这条链路是反复真机踩坑后固化的，每一步都不能省，原因见 docs/android-release.md：
#   1. cargo build --target aarch64-linux-android 产出 Rust 静态库（V2 渲染内核）；
#   2. 用仓库自带的 arm64 libyogacore.a 临时替换 gomobile 模块缓存里的 x86-64 版
#      （gox-backend-skia 的 cgo 布局依赖它，不换则 arm64 链接失败），构建后恢复；
#   3. gomobile build 出裸 APK；
#   4. pack_apk.py 做 STORED 重打包 + targetSdk=28 manifest + 注入 libc++_shared.so；
#   5. zipalign -p 4 对齐；
#   6. apksigner 用仓库自带的 gomobile debug keystore 重签（保证与历史版本签名一致，
#      用户真机可覆盖安装，不弹「签名冲突」）。
#
# 用法（在任意目录）：
#   bash scripts/android/build-release.sh                       # 默认 arm64
#   bash scripts/android/build-release.sh --abi x86_64           # x86_64
#   bash scripts/android/build-release.sh --out /path/app.apk
#   ANDROID_NDK_HOME=... ANDROID_HOME=... bash scripts/android/build-release.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"         # .../gox-backend/scripts/android
BACKEND_ROOT="$(cd "$HERE/../.." && pwd)"                    # .../gox-backend
PLAYGROUND="$(cd "$BACKEND_ROOT/../gox" && pwd)"             # .../gox（装配入口所在仓）
ASSETS="$HERE/assets"

# ---------- 参数 ----------
ABI="arm64-v8a"
OUT=""
RAW=""
USE_RAW=0
SKIP_RUST=0
while [ $# -gt 0 ]; do
  case "$1" in
    --abi)       ABI="$2"; shift 2 ;;
    --out)       OUT="$2"; shift 2 ;;
    --raw)       RAW="$2"; USE_RAW=1; shift 2 ;;   # 复用已有裸 APK，跳过 Rust/gomobile 构建
    --skip-rust) SKIP_RUST=1; shift ;;
    -h|--help)   sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "[android] 未知参数: $1" >&2; exit 1 ;;
  esac
done

case "$ABI" in
  arm64-v8a) GOARCH=arm64; NDK_TRIPLE=aarch64-linux-android; RUST_TARGET=aarch64-linux-android ;;
  x86_64)    GOARCH=amd64; NDK_TRIPLE=x86_64-linux-android;  RUST_TARGET=x86_64-linux-android ;;
  *) echo "[android] 不支持的 ABI: $ABI（仅 arm64-v8a / x86_64）" >&2; exit 1 ;;
esac

if [ -z "$OUT" ]; then
  OUT="$BACKEND_ROOT/GX-Release-$ABI.apk"
fi
if [ "$USE_RAW" -eq 1 ]; then
  [ -f "$RAW" ] || { echo "[android] ERROR: --raw 指定的裸 APK 不存在: $RAW" >&2; exit 1; }
  SKIP_RUST=1   # 复用裸 APK 时无需重编 Rust
else
  RAW="/tmp/gx-release-raw-$ABI.apk"
fi

# ---------- 工具链探测 ----------
detect_env() {
  if [ -z "${ANDROID_HOME:-}" ] || [ ! -d "${ANDROID_HOME:-}" ]; then
    for d in /tmp/android-sdk "$HOME/Android/Sdk" /opt/android-sdk /usr/lib/android-sdk; do
      [ -d "$d" ] && { export ANDROID_HOME="$d"; break; }
    done
  fi
  if [ -z "${ANDROID_NDK_HOME:-}" ] || [ ! -d "${ANDROID_NDK_HOME:-}" ]; then
    for d in /tmp/android-ndk-* "$HOME/Android/Sdk/ndk"/* /opt/android-ndk*; do
      [ -d "$d" ] && { export ANDROID_NDK_HOME="$d"; break; }
    done
  fi
  export ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
  [ -n "${ANDROID_HOME:-}" ] || { echo "[android] ERROR: 找不到 ANDROID_HOME" >&2; exit 1; }
  [ -n "${ANDROID_NDK_HOME:-}" ] || { echo "[android] ERROR: 找不到 ANDROID_NDK_HOME" >&2; exit 1; }

  # build-tools：取已装的最高版本
  BT=""
  if [ -d "$ANDROID_HOME/build-tools" ]; then
    BT="$(ls -1 "$ANDROID_HOME/build-tools" | sort -V | tail -1)"
    BT="$ANDROID_HOME/build-tools/$BT"
  fi
  [ -n "$BT" ] && [ -x "$BT/zipalign" ] && [ -x "$BT/apksigner" ] \
    || { echo "[android] ERROR: build-tools 缺 zipalign/apksigner: $ANDROID_HOME/build-tools" >&2; exit 1; }

  GOMOBILE="$(command -v gomobile || true)"
  [ -z "$GOMOBILE" ] && [ -x /usr/local/go-packages/bin/gomobile ] && GOMOBILE=/usr/local/go-packages/bin/gomobile
  [ -n "$GOMOBILE" ] || { echo "[android] ERROR: 找不到 gomobile（go install golang.org/x/mobile/cmd/gomobile@latest）" >&2; exit 1; }

  echo "[android] ANDROID_HOME=$ANDROID_HOME"
  echo "[android] NDK=$ANDROID_NDK_HOME"
  echo "[android] build-tools=$BT"
  echo "[android] gomobile=$GOMOBILE"
}
detect_env

# ---------- Rust 静态库（V2 渲染内核）----------
if [ "$SKIP_RUST" -eq 0 ]; then
  echo "[android] === 1/6 编译 Rust 静态库 ($RUST_TARGET) ==="
  (cd "$BACKEND_ROOT/rust/rust" && \
    ANDROID_NDK_HOME="$ANDROID_NDK_HOME" cargo build --release --target "$RUST_TARGET")
  # 剥 LTO section：Go 1.24+ cgo 拒绝带 .llvmbc/.llvmcmd 的 .a（与 build.sh 同款逻辑）。
  LIBDIR="$BACKEND_ROOT/rust/rust/target/$RUST_TARGET/release"
  for lib in "$LIBDIR"/libgxbackend_rust.a "$LIBDIR"/deps/libgxbackend_rust-*.a "$LIBDIR"/deps/libgx_gpu-*.a; do
    [ -f "$lib" ] || continue
    if objdump -h "$lib" 2>/dev/null | grep -qE '\.llvmbc|\.llvmcmd'; then
      echo "[android] strip LTO sections: $(basename "$lib")"
      objcopy --remove-section=.llvmbc --remove-section=.llvmcmd "$lib"
    fi
  done
else
  echo "[android] === 1/6 跳过 Rust 编译（--skip-rust） ==="
fi

# ---------- yoga arm64 静态库临时替换 ----------
YOGA_LIB=""
YOGA_BAK=""
restore_yoga() {
  if [ -n "$YOGA_BAK" ] && [ -f "$YOGA_BAK" ]; then
    cp -f "$YOGA_BAK" "$YOGA_LIB"
    rm -f "$YOGA_BAK"
    echo "[android] yoga 模块缓存已恢复"
  fi
}
trap restore_yoga EXIT

if [ "$USE_RAW" -eq 1 ]; then
  echo "[android] === 2/6 跳过 yoga 替换（复用裸 APK） ==="
elif [ "$GOARCH" = "arm64" ]; then
  echo "[android] === 2/6 临时替换 yoga arm64 静态库 ==="
  YOGA_SRC="$(go env GOPATH 2>/dev/null)/pkg/mod/github.com/ahati/yoga-go-binding@v0.1.0/lib/libyogacore.a"
  if [ ! -f "$YOGA_SRC" ]; then
    # 兜底：常见 GOPATH 位置
    YOGA_SRC=/usr/local/go-packages/pkg/mod/github.com/ahati/yoga-go-binding@v0.1.0/lib/libyogacore.a
  fi
  if [ ! -f "$YOGA_SRC" ]; then
    echo "[android] ERROR: 找不到 yoga-go-binding 的 libyogacore.a，请先 go mod download" >&2
    exit 1
  fi
  YOGA_LIB="$YOGA_SRC"
  YOGA_BAK="${YOGA_LIB}.gx-bak"
  cp -f "$YOGA_LIB" "$YOGA_BAK"
  cp -f "$ASSETS/libyogacore-arm64.a" "$YOGA_LIB"
  echo "[android] yoga -> arm64 (备份: $YOGA_BAK)"
else
  echo "[android] === 2/6 x86_64 用模块自带 yoga 库，跳过替换 ==="
fi

# ---------- gomobile build ----------
if [ "$USE_RAW" -eq 1 ]; then
  echo "[android] === 3/6 跳过 gomobile build（复用裸 APK） ==="
else
  echo "[android] === 3/6 gomobile build ($GOARCH) ==="
  cd "$PLAYGROUND"
  export ANDROID_HOME ANDROID_SDK_ROOT ANDROID_NDK_HOME
  export GOTOOLCHAIN="${GOTOOLCHAIN:-go1.27.4}"
  export CGO_ENABLED=1
  "$GOMOBILE" build -target="android/$GOARCH" -androidapi=24 -o "$RAW" ./examples/playground/android
  echo "[android] 裸 APK: $RAW ($(stat -c%s "$RAW") bytes)"
fi
[ -f "$RAW" ] || { echo "[android] ERROR: 裸 APK 不存在: $RAW" >&2; exit 1; }

# 恢复 yoga（不等到 trap，尽早还原现场）
restore_yoga

# ---------- 后处理：STORED + manifest + libc++ + 去签名 ----------
echo "[android] === 4/6 APK 后处理 ==="
UNSIGNED="${RAW%.apk}-unsigned.apk"
LIBCXX="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$NDK_TRIPLE/libc++_shared.so"
[ -f "$LIBCXX" ] || { echo "[android] ERROR: 找不到 libc++_shared.so: $LIBCXX" >&2; exit 1; }
python3 "$HERE/pack_apk.py" "$RAW" "$UNSIGNED" \
  --manifest "$ASSETS/AndroidManifest.xml" \
  --libcxx "$LIBCXX" \
  --abi "$ABI"

# ---------- zipalign ----------
echo "[android] === 5/6 zipalign -p 4 ==="
ALIGNED="${RAW%.apk}-aligned.apk"
"$BT/zipalign" -p -f 4 "$UNSIGNED" "$ALIGNED"

# ---------- apksigner 重签 ----------
echo "[android] === 6/6 apksigner 签名 ==="
KS="$ASSETS/gomobile-debug.p12"
"$BT/apksigner" sign \
  --ks "$KS" --ks-key-alias androiddebugkey \
  --ks-pass pass:android --key-pass pass:android \
  --v1-signing-enabled true --v2-signing-enabled true --v3-signing-enabled true \
  --out "$OUT" "$ALIGNED"

# ---------- 校验 ----------
echo "[android] === 校验 ==="
"$BT/apksigner" verify --verbose --print-certs "$OUT" 2>&1 | grep -E 'Verifies|scheme|Signer|SHA-256' | head -10 || true
"$BT/zipalign" -c -p 4 "$OUT" && echo "[android] zipalign OK"
"$BT/aapt" dump badging "$OUT" 2>/dev/null | grep -E '^(package|sdkVersion|targetSdkVersion|native-code)' || true
echo "[android] ✅ 交付: $OUT ($(stat -c%s "$OUT") bytes)"
