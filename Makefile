# GoX 后端实现构建与测试入口（需 Go 1.27+）
# 本仓含 Rust/Skia/Web/GPU 后端与 Skia 绑定，只依赖相邻 gox-core 仓的 gxapp.Backend 契约。
GO := go
TOOLCHAIN := GOTOOLCHAIN=go1.27.4

# 本仓含 Go 源码的模块（顶层无 module，仅 go.work 容器）。
MODULES := rust/go skia web third_party/skia
# Rust 静态库构建脚本（cgo 链接需要）。
RUST_BUILD := rust/rust/build.sh

.PHONY: build test test-rust clean all build-android-release

all: build

## build-rust: 编译 Rust 静态库并剥离 LTO section
build-rust:
	bash $(RUST_BUILD)

## build: 编译 Rust 静态库 + 校验 Go 模块
build: build-rust
	@set -e; for m in $(MODULES); do \
	  echo "==> $$m"; \
	  (cd $$m && $(TOOLCHAIN) $(GO) build ./...) || exit 1; \
	done
	@echo "构建完成"

## test: 逐个模块运行测试（需先 build-rust 产出静态库）
test:
	@set -e; for m in $(MODULES); do \
	  echo "==> $$m"; \
	  (cd $$m && $(TOOLCHAIN) $(GO) test ./...) || exit 1; \
	done
	@echo "全部模块测试通过"

## test-rust: Rust crate 测试
test-rust:
	cargo test --manifest-path rust/rust/Cargo.toml
	cargo test --manifest-path gpu/Cargo.toml

## build-android-release: 一键构建 Android Release APK（详见 docs/android-release.md）
##   默认 arm64；可用 make build-android-release ABI=x86_64 OUT=/tmp/x.apk
ABI ?= arm64-v8a
build-android-release:
	bash scripts/android/build-release.sh --abi $(ABI) $(if $(OUT),--out $(OUT),) $(if $(RAW),--raw $(RAW),)

## clean: 清理 Rust 构建产物
clean:
	rm -rf rust/rust/target gpu/target
