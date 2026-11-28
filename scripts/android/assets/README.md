# Android 构建资产

本目录存放 Android Release 构建所需的固定资产，均为非敏感、可复现用途：

| 文件 | 用途 | 来源 / 说明 |
|---|---|---|
| `gomobile-debug.p12` | APK 签名（重打包后必须重签） | `CN=gomobile` 的 **debug** 证书，alias `androiddebugkey`，密码 `android`，SHA-256 `70a6f8797dba0bad15c4d88a087279bdee8edbdc6b57bad36f9ff6cfc24991e0`。与历史交付 APK 同签名，保证用户真机可覆盖安装。仅 debug 用途，不含正式发布密钥。 |
| `AndroidManifest.xml` | 替换 gomobile 默认 manifest | 二进制 AXML，`targetSdkVersion=28`。避免 Android 14+ 安装时弹「为旧版制作」。从历史可正常安装的 APK 提取。 |
| `libyogacore-arm64.a` | arm64 架构的 Yoga 布局库 | Facebook Yoga 3.x（C++20），`file` 显示 `ELF 64-bit ... ARM aarch64`。替代 Go 模块缓存里自带的 x86-64 版本，供 arm64 交叉编译链接。构建脚本临时替换、事后自动恢复。 |

这些文件是构建链路的一部分，不要删除。更新签名证书时务必确认新证书与线上版本指纹一致，否则用户覆盖安装会失败。
