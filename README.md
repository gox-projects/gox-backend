# gox-backend

GOX 的渲染后端实现仓。从 [gox](https://github.com/robertpanvip/gox) 仓拆出，与 [gox-core](https://github.com/robertpanvip/gox-core) 共同构成三仓布局。

## 目录结构

```
gox-backend/
├── rust/        Rust 后端（Go cgo Adapter + Rust Runtime + Wire 协议）
│   ├── go/          module gox-backend-rust（Go Adapter）
│   ├── rust/        Rust staticlib（libgxbackend_rust）
│   └── android/     module gox-backend-android（Android/V2 能力层）
├── skia/        Skia 后端（module gox-backend-skia）
├── web/         Web/wasm 后端（module gox-backend-web）
├── gpu/         GPU 抽象（gx_gpu，Rust crate）
├── third_party/skia/  Skia 绑定（module gox-skia）
├── testdata/threeway/ 三端一致性用例（skia/web/rust 共享）
└── scripts/           threeway-parity.sh 三端一致性校验
```

各后端只依赖 `gox-core` 的 `gxapp.Backend` 契约，core 对后端零编译依赖。

## 仓库布局

三仓需相邻放置（同一父目录下），`go.mod` 用相对路径 `replace` 互相引用：

```
<parent>/
├── gox-core/
├── gox-backend/     ← 本仓
└── gox/
```

## 构建与测试

需 Go 1.27+ 与 Rust toolchain。

```bash
make build        # 编译 Rust 静态库 + 校验各 Go 模块
make test         # 各 Go 模块测试（需先 build 产出静态库）
make test-rust    # Rust crate 测试
make build-rust   # 仅编译 Rust 静态库
```

Rust 静态库构建后需剥离 LLVM bitcode section（Go 1.24+ 的 cgo 安全检查要求），`rust/rust/build.sh` 已自动处理。

三端一致性校验（Web / Skia / Rust）：

```bash
bash scripts/threeway-parity.sh
```

## 子模块文档

- [Rust 后端协议](rust/PROTOCOL.md) · [Rust 后端基准](rust/BENCHMARK.md)
- [Skia 后端](skia/README.md) · [GPU 抽象](gpu/README.md)
