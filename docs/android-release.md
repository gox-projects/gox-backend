# Android Release APK 构建指南

GX Playground 的 Android APK 用 [gomobile](https://pkg.go.dev/golang.org/x/mobile/cmd/gomobile) 打包，渲染走 `gox-backend-android`（Rust V2 内核 + 布局/测量回退）。本文记录一键构建方式，以及每一步背后的原因——**这些步骤都是真机踩坑验证过的，不要凭直觉删减**。

## 一键构建

```bash
# 默认 arm64（真机/平板绝大多数）
bash scripts/android/build-release.sh

# 指定输出路径
bash scripts/android/build-release.sh --out /tmp/GX.apk

# x86_64（模拟器）
bash scripts/android/build-release.sh --abi x86_64

# 复用已有裸 APK，只重跑后处理+签名（调试打包流程时用）
bash scripts/android/build-release.sh --raw /tmp/gx-release-raw-arm64-v8a.apk
```

脚本自动完成 6 步：Rust 静态库交叉编译 → yoga 库临时替换 → gomobile build → STORED 重打包 → zipalign → apksigner 重签，最后自动验签 + 打印包信息。产物默认落在仓库根目录 `GX-Release-<ABI>.apk`。

## 依赖工具链

| 依赖 | 版本/要求 | 说明 |
|---|---|---|
| Go | 1.27.4（GOTOOLCHAIN） | playground 及各 backend 模块 |
| Rust | 1.93+ | 需装 target：`rustup target add aarch64-linux-android x86_64-linux-android` |
| Android SDK | build-tools ≥ 34 | 提供 `zipalign` / `apksigner` / `aapt` |
| Android NDK | r27c | 提供 `libc++_shared.so` 与 cgo 交叉编译 |
| gomobile | 与 `golang.org/x/mobile` 同版本 | `go install golang.org/x/mobile/cmd/gomobile@latest` |
| JDK | 17+ | apksigner 需要 |
| Python | 3.10+ | 跑 `pack_apk.py`（仅标准库） |

脚本会按以下顺序自动探测 `ANDROID_HOME` / `ANDROID_NDK_HOME`，也可显式用环境变量指定：

```
ANDROID_HOME: $ANDROID_HOME → /tmp/android-sdk → ~/Android/Sdk → /opt/android-sdk → /usr/lib/android-sdk
NDK:          $ANDROID_NDK_HOME → /tmp/android-ndk-* → ~/Android/Sdk/ndk/* → /opt/android-ndk*
```

## 为什么需要这几步（重点）

### 1. Rust 静态库 + 剥 LTO section

V2 渲染内核在 `rust/rust`，cgo 静态链接。Go 1.24+ 的 cgo 安全检查会拒绝带 `.llvmbc` / `.llvmcmd` section 的 `.a`（LTO 中间表示），所以 cargo build 后必须 `objcopy --remove-section`。脚本已内置，`rust/rust/build.sh` 桌面版也是同款逻辑。

### 2. yoga arm64 静态库替换 —— 不是可选

有人会问：Android 不是走 Rust 桥吗，为什么还要动 Yoga？因为 **Android 端目前还不是纯 Rust**：

```
playground/android (Go 装配)
  ├─ gox-backend-android   → Rust 桥：wgpu/glyphon 直通 ANativeWindow
  └─ gox-backend-skia      → 布局 / 命中框 / 字体测量（cgo）
       └─ yoga-go-binding  → libyogacore.a（C++20）
```

`gox-backend-skia/layout.go` 用 Yoga 做 Flexbox 布局，`window.go` 的 V2 路径调 `skia.PaintToCachedScaled` 出命中框，`inject_width.go` 用 `skia.MeasureTextWidth` 测编辑器字体宽度——这些都是 cgo 调用，链接期必须要有对应架构的 `libyogacore.a`。而 Go 模块缓存里那份是 **x86-64** 的（`ELF 64-bit ... x86-64`），arm64 交叉编译直接链接失败。

`assets/libyogacore-arm64.a` 是 arm64 版（`ELF 64-bit ... ARM aarch64`）。脚本会在构建前临时替换模块缓存里的那份，构建后自动恢复（trap + 显式恢复双保险）。除非把布局/测量/命中框全部搬进 Rust，否则这步省不掉。

### 3. STORED 重打包 + 注入 libc++ + targetSdk=28

`pack_apk.py` 做四件事，每件都对应一个真机故障：

- **全条目 ZIP_STORED**：DEFLATE 压缩 so/dex 会导致安装/启动失败（`libc++_shared.so not found`、闪退）。Android 要求 so 直接 mmap。
- **注入 libc++_shared.so**：rust/go 经 cgo 依赖它，gomobile 不会自动打包，需从 NDK sysroot 按 ABI 注入。
- **替换 targetSdk=28 manifest**：gomobile 默认 manifest targetSdk 过低，Android 14+ 安装弹「为旧版制作，可能无法正常运行」。
- **去旧签名条目 + 时间戳归一化**：重打包后原签名失效，去掉 `META-INF/*.SF/.RSA/...` 交给外层重签；早于 1980 的时间戳会让部分 zipalign/apksigner 报错。

### 4. apksigner 用固定 keystore 重签

`assets/gomobile-debug.p12` 是 `CN=gomobile` 的 debug 证书（`alias=androiddebugkey`，密码 `android`，SHA-256 `70a6f879...91e0`），与历史交付 APK 同签名。**必须用同一把钥匙重签**，否则用户真机覆盖安装时报「签名冲突，请先卸载旧版」。这是 debug 用途，不涉及正式发布密钥。

## 校验

脚本末尾自动跑：

```bash
apksigner verify --verbose --print-certs GX-Release-arm64-v8a.apk   # v1/v2/v3 全 green
zipalign -c -p 4 GX-Release-arm64-v8a.apk                          # OK
aapt dump badging GX-Release-arm64-v8a.apk                          # targetSdkVersion=28, native-code=arm64-v8a
```

## FAQ

**Q：能不能直接用 GitHub Actions 出包？**
可以，本仓 `.github/workflows/release.yml` 已有 Rust 静态库三平台发布；Android APK 需额外装 SDK/NDK/JDK/gomobile，并在 CI 里 checkout 相邻的 `gox` 仓（装配入口 `examples/playground/android` 在那一侧）。本地脚本的 6 步可以直接搬进 workflow。

**Q：为什么 `gox` 仓在 `../gox`？**
APK 入口是 `gox/examples/playground/android/main.go`，它 import 本仓的 `gox-backend-android`。两个仓按兄弟目录检出即可（`.../gox` 与 `.../gox-backend`），脚本据此定位。

**Q：改了 Rust 代码后要重打吗？**
要。`--skip-rust` 只在你确认静态库已最新时用，否则会打到旧内核。
