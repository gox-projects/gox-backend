#!/usr/bin/env python3
"""APK 后处理：把 gomobile build 的裸 APK 变成可覆盖安装的交付 APK。

背景（都是真机踩过的坑，勿删）：
  1. gomobile build 产出的 libandroid.so / classes.dex 若被普通 DEFLATE 压缩，
     Android 无法直接 mmap so，安装期报 "libc++_shared.so not found" 或启动即闪退。
     → 所有条目必须 ZIP_STORED（0% 压缩）。
  2. rust/go 经 cgo 链接了 libc++_shared.so，gomobile 不会自动把它打进 APK。
     → 从 NDK sysroot 注入对应 ABI 的 libc++_shared.so。
  3. gomobile 默认 manifest 的 targetSdk 过低，Android 14+ 安装弹「为旧版制作」。
     → 换成 targetSdk=28 的 manifest（assets/AndroidManifest.xml）。
  4. 重打包后原签名失效，且带旧签名条目会被 apksigner 拒绝。
     → 去掉 META-INF 下的 .SF/.RSA/.DSA/.EC/MANIFEST.MF，交给外层 apksigner 重签。
  5. ZIP 时间戳早于 1980-01-01 时部分 zipalign/apksigner 版本会报错。
     → 归一化到 1980-01-01。

用法：
  python3 pack_apk.py RAW.apk OUT.apk \
      --manifest  assets/AndroidManifest.xml \
      --libcxx    <NDK>/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so \
      --abi       arm64-v8a
"""
import argparse
import os
import sys
import zipfile

SIG_EXTS = ('.SF', '.RSA', '.DSA', '.EC')
MIN_DATE = (1980, 1, 1, 0, 0, 0)


def is_signature_entry(name: str) -> bool:
    up = name.upper()
    if not up.startswith('META-INF/'):
        return False
    return up.endswith(SIG_EXTS) or up == 'META-INF/MANIFEST.MF'


def repack(raw: str, out: str, manifest: str | None, libcxx: str | None, abi: str) -> None:
    manifest_bytes = open(manifest, 'rb').read() if manifest else None
    libcxx_bytes = open(libcxx, 'rb').read() if libcxx else None
    libcxx_entry = f'lib/{abi}/libc++_shared.so'

    removed = []
    with zipfile.ZipFile(raw, 'r') as zin, zipfile.ZipFile(out, 'w') as zout:
        for item in zin.infolist():
            name = item.filename
            if is_signature_entry(name):
                removed.append(name)
                continue

            if name == 'AndroidManifest.xml' and manifest_bytes is not None:
                data = manifest_bytes
            else:
                data = zin.read(name)

            date = item.date_time
            if date[0] < 1980:
                date = MIN_DATE
            zi = zipfile.ZipInfo(name, date_time=date)
            zi.compress_type = zipfile.ZIP_STORED
            zi.external_attr = item.external_attr
            zi.create_system = item.create_system
            zout.writestr(zi, data)

        if libcxx_bytes is not None:
            # 幂等：若 raw 里已注入过则不再重复写（条目名精确匹配）。
            with zipfile.ZipFile(raw, 'r') as zcheck:
                already = libcxx_entry in zcheck.namelist()
            if not already:
                zi = zipfile.ZipInfo(libcxx_entry, date_time=MIN_DATE)
                zi.compress_type = zipfile.ZIP_STORED
                zout.writestr(zi, libcxx_bytes)

    print(f'[pack] removed signatures: {removed}')
    print(f'[pack] manifest override: {bool(manifest_bytes)}, libc++ injected: {bool(libcxx_bytes) and not already if libcxx_bytes else False}')
    print(f'[pack] written: {out} ({os.path.getsize(out)} bytes)')


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('raw', help='gomobile build 产出的裸 APK')
    ap.add_argument('out', help='输出 APK（未签名）')
    ap.add_argument('--manifest', help='替换用的 AndroidManifest.xml（targetSdk=28）')
    ap.add_argument('--libcxx', help='NDK 的 libc++_shared.so 路径')
    ap.add_argument('--abi', default='arm64-v8a',
                    choices=['arm64-v8a', 'x86_64'],
                    help='so 注入的目标 ABI（默认 arm64-v8a）')
    args = ap.parse_args()

    if not os.path.isfile(args.raw):
        print(f'[pack] ERROR: 裸 APK 不存在: {args.raw}', file=sys.stderr)
        return 1
    for label, path in (('manifest', args.manifest), ('libc++', args.libcxx)):
        if path and not os.path.isfile(path):
            print(f'[pack] ERROR: {label} 不存在: {path}', file=sys.stderr)
            return 1

    repack(args.raw, args.out, args.manifest, args.libcxx, args.abi)
    return 0


if __name__ == '__main__':
    sys.exit(main())
