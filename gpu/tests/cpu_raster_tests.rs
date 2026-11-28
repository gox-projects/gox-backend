//! CPU 软光栅后端的像素级契约测试（仅 cpu feature 构建下编译运行）。
//!
//! 覆盖 tiny-skia 后端的实际绘制语义：清屏、纯色/半透明 quad、圆角 clamp、
//! 描边、渐变方向与色标排序、纹理 UV 窗口/key 去重/越界/圆角遮罩/整体 alpha，
//! 以及 C ABI 在 CPU 构建下的完整 begin→upload→quad→end→read_rgba 帧流程。

#![cfg(feature = "cpu")]

use gx_gpu::capi::{
    gx_gpu_backend_kind, gx_gpu_begin, gx_gpu_end, gx_gpu_free, gx_gpu_new, gx_gpu_quad,
    gx_gpu_read_rgba, gx_gpu_resize, gx_gpu_tex, gx_gpu_upload_rgba, CQuad, CTex, GX_GPU_KIND_CPU,
};
use gx_gpu::cpu::CpuBackend;
use gx_gpu::{GpuFrame, Grad, TexUpload};
use std::sync::Arc;

fn near(a: u8, b: u8, tol: i32) -> bool {
    (a as i32 - b as i32).abs() <= tol
}

fn solid_tex_data(w: u32, h: u32, rgba: [u8; 4]) -> Arc<Vec<u8>> {
    let mut data = vec![0u8; (w * h * 4) as usize];
    for px in data.chunks_exact_mut(4) {
        px.copy_from_slice(&rgba);
    }
    Arc::new(data)
}

// ---------------------------------------------------------------------------
// 清屏与纯色 quad
// ---------------------------------------------------------------------------

#[test]
fn clear_color_fills_every_pixel() {
    let mut b = CpuBackend::new();
    let f = GpuFrame::new(6, 6, [10.0, 20.0, 30.0, 255.0]);
    let pm = b.render(&f);
    for y in 0..6 {
        for x in 0..6 {
            let p = pm.pixel(x, y).unwrap();
            assert!(near(p.red(), 10, 1) && near(p.green(), 20, 1) && near(p.blue(), 30, 1),
                "清屏色应在 ({x},{y}) 铺满，实得 {:?}", p);
            assert_eq!(p.alpha(), 255);
        }
    }
}

#[test]
fn solid_quad_writes_exact_rgba_at_center() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(10, 10, [0.0, 0.0, 0.0, 255.0]);
    f.quad([2.0, 2.0, 6.0, 6.0], [0.0; 4], [200.0, 100.0, 50.0, 255.0], 0.0, 0.0);
    let pm = b.render(&f);
    let p = pm.pixel(5, 5).unwrap();
    assert!(near(p.red(), 200, 1) && near(p.green(), 100, 1) && near(p.blue(), 50, 1),
        "quad 中心应是纯色，实得 {:?}", p);
}

#[test]
fn semi_transparent_quad_blends_in_premultiplied_space() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(10, 10, [0.0, 0.0, 0.0, 255.0]);
    // 红 alpha=128 覆盖不透明黑：预乘口径下红通道 ≈ 128，alpha 仍为 255。
    f.quad([0.0, 0.0, 10.0, 10.0], [0.0; 4], [255.0, 0.0, 0.0, 128.0], 0.0, 0.0);
    let pm = b.render(&f);
    let p = pm.pixel(5, 5).unwrap();
    assert!(near(p.red(), 128, 20), "半透明红中心应≈128，实得 {:?}", p);
    assert!(near(p.alpha(), 255, 3), "不透明底 + 半透明叠层 alpha 仍应≈255，实得 {:?}", p);
}

#[test]
fn zero_alpha_quad_is_skipped() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(6, 6, [1.0, 2.0, 3.0, 255.0]);
    f.quad([0.0, 0.0, 6.0, 6.0], [0.0; 4], [255.0, 0.0, 0.0, 0.0], 0.0, 0.0);
    let pm = b.render(&f);
    let p = pm.pixel(3, 3).unwrap();
    assert!(near(p.red(), 1, 1) && near(p.green(), 2, 1) && near(p.blue(), 3, 1),
        "alpha=0 的 quad 不应改变清屏色，实得 {:?}", p);
}

#[test]
fn zero_size_quad_is_skipped() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(6, 6, [0.0, 0.0, 0.0, 255.0]);
    f.quad([1.0, 1.0, 0.0, 4.0], [0.0; 4], [255.0, 0.0, 0.0, 255.0], 0.0, 0.0);
    f.quad([1.0, 1.0, 4.0, 0.0], [0.0; 4], [255.0, 0.0, 0.0, 255.0], 0.0, 0.0);
    let pm = b.render(&f);
    assert_eq!(pm.pixel(3, 3).unwrap().red(), 0, "零宽/零高 quad 必须跳过");
}

#[test]
fn oversized_radius_is_clamped_to_half_dimension() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(8, 8, [0.0, 0.0, 0.0, 255.0]);
    // radius=100 > 8/2，应 clamp 到 4 → 外观等同最大圆角矩形。
    f.quad([0.0, 0.0, 8.0, 8.0], [0.0; 4], [255.0, 255.0, 255.0, 255.0], 100.0, 0.0);
    let pm = b.render(&f);
    assert_eq!(pm.pixel(0, 0).unwrap().red(), 0, "clamp 后四角仍应是清屏色");
    assert_eq!(pm.pixel(7, 7).unwrap().red(), 0, "右下角同理");
    assert_eq!(pm.pixel(4, 4).unwrap().red(), 255, "中心应填满");
}

// ---------------------------------------------------------------------------
// 描边
// ---------------------------------------------------------------------------

#[test]
fn border_ring_leaves_interior_untouched() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(20, 20, [0.0, 0.0, 0.0, 255.0]);
    f.quad([2.0, 2.0, 16.0, 16.0], [0.0; 4], [0.0, 255.0, 0.0, 255.0], 0.0, 2.0);
    let pm = b.render(&f);
    assert_eq!(pm.pixel(10, 10).unwrap().green(), 0, "描边环内部应透明");
    assert!(pm.pixel(2, 10).unwrap().green() > 100, "左边框应着色");
    assert!(pm.pixel(17, 10).unwrap().green() > 100, "右边框应着色");
}

// ---------------------------------------------------------------------------
// 渐变
// ---------------------------------------------------------------------------

#[test]
fn horizontal_gradient_goes_left_to_right() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(100, 10, [0.0, 0.0, 0.0, 255.0]);
    // CSS 90° = 向右：dir=(1,0)，stop0 在左（红），stop1 在右（蓝）。
    f.gradient(
        [0.0, 0.0, 100.0, 10.0], [0.0; 4], 0.0,
        Grad {
            dx: 1.0, dy: 0.0,
            stops: vec![[0.0, 255.0, 0.0, 0.0, 255.0], [1.0, 0.0, 0.0, 255.0, 255.0]],
        },
    );
    let pm = b.render(&f);
    let left = pm.pixel(3, 5).unwrap();
    let right = pm.pixel(96, 5).unwrap();
    assert!(left.red() > 180, "左端应是 stop0 红: {:?}", left);
    assert!(right.blue() > 180, "右端应是 stop1 蓝: {:?}", right);
}

#[test]
fn gradient_stops_are_sorted_by_position() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(100, 10, [0.0, 0.0, 0.0, 255.0]);
    // 故意逆序传入色标：CPU 后端应先按 pos 排序（与 GPU uniform 表同口径）。
    f.gradient(
        [0.0, 0.0, 100.0, 10.0], [0.0; 4], 0.0,
        Grad {
            dx: 1.0, dy: 0.0,
            stops: vec![
                [1.0, 0.0, 0.0, 255.0, 255.0],
                [0.0, 255.0, 0.0, 0.0, 255.0],
            ],
        },
    );
    let pm = b.render(&f);
    assert!(pm.pixel(3, 5).unwrap().red() > 180, "排序后左端应是红");
    assert!(pm.pixel(96, 5).unwrap().blue() > 180, "排序后右端应是蓝");
}

#[test]
fn gradient_with_empty_stops_is_skipped() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(10, 10, [7.0, 8.0, 9.0, 255.0]);
    f.gradient(
        [0.0, 0.0, 10.0, 10.0], [0.0; 4], 0.0,
        Grad { dx: 1.0, dy: 0.0, stops: vec![] },
    );
    let pm = b.render(&f);
    assert!(near(pm.pixel(5, 5).unwrap().red(), 7, 1), "空色标渐变不应绘制");
}

#[test]
fn gradient_with_zero_direction_is_skipped() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(10, 10, [7.0, 8.0, 9.0, 255.0]);
    f.gradient(
        [0.0, 0.0, 10.0, 10.0], [0.0; 4], 0.0,
        Grad {
            dx: 0.0, dy: 0.0,
            stops: vec![[0.0, 255.0, 0.0, 0.0, 255.0]],
        },
    );
    let pm = b.render(&f);
    assert!(near(pm.pixel(5, 5).unwrap().red(), 7, 1), "零方向渐变不应绘制（上层语义不产生）");
}

// ---------------------------------------------------------------------------
// 纹理
// ---------------------------------------------------------------------------

#[test]
fn texture_uv_window_samples_correct_quadrant() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(16, 8, [0.0, 0.0, 0.0, 255.0]);
    // 源 8x8：左上红、右下蓝。
    let mut data = vec![0u8; 8 * 8 * 4];
    for y in 0..8usize {
        for x in 0..8usize {
            let px = &mut data[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
            if x < 4 && y < 4 {
                px.copy_from_slice(&[255, 0, 0, 255]);
            } else if x >= 4 && y >= 4 {
                px.copy_from_slice(&[0, 0, 255, 255]);
            } else {
                px.copy_from_slice(&[0, 255, 0, 255]);
            }
        }
    }
    f.uploads.push(TexUpload { id: 1, key: 1, w: 8, h: 8, data: Arc::new(data), premul: true });
    // 左半屏取左上窗口（红），右半屏取右下窗口（蓝）。
    f.tex([0.0, 0.0, 8.0, 8.0], [0.0; 4], 1, [0.0, 0.0, 4.0, 4.0], 0.0, 1.0);
    f.tex([8.0, 0.0, 8.0, 8.0], [0.0; 4], 1, [4.0, 4.0, 8.0, 8.0], 0.0, 1.0);
    let pm = b.render(&f);
    assert!(pm.pixel(3, 3).unwrap().red() > 200, "左上窗口应采样到红: {:?}", pm.pixel(3, 3).unwrap());
    assert!(pm.pixel(12, 4).unwrap().blue() > 200, "右下窗口应采样到蓝: {:?}", pm.pixel(12, 4).unwrap());
}

#[test]
fn texture_key_change_rebuilds_cached_pixmap() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(4, 4, [0.0; 4]);
    f.uploads.push(TexUpload { id: 5, key: 1, w: 4, h: 4, data: solid_tex_data(4, 4, [255, 0, 0, 255]), premul: true });
    f.tex([0.0, 0.0, 4.0, 4.0], [0.0; 4], 5, [0.0, 0.0, 4.0, 4.0], 0.0, 1.0);
    let pm1 = b.render(&f);
    assert!(pm1.pixel(2, 2).unwrap().red() > 200, "首帧应是红");

    let mut f2 = GpuFrame::new(4, 4, [0.0; 4]);
    f2.uploads.push(TexUpload { id: 5, key: 2, w: 4, h: 4, data: solid_tex_data(4, 4, [0, 0, 255, 255]), premul: true });
    f2.tex([0.0, 0.0, 4.0, 4.0], [0.0; 4], 5, [0.0, 0.0, 4.0, 4.0], 0.0, 1.0);
    let pm2 = b.render(&f2);
    assert!(pm2.pixel(2, 2).unwrap().blue() > 200, "key 变化后应重建为蓝: {:?}", pm2.pixel(2, 2).unwrap());
}

#[test]
fn texture_reference_without_upload_is_skipped() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(6, 6, [3.0, 4.0, 5.0, 255.0]);
    f.tex([0.0, 0.0, 6.0, 6.0], [0.0; 4], 99, [0.0, 0.0, 4.0, 4.0], 0.0, 1.0);
    let pm = b.render(&f);
    assert!(near(pm.pixel(3, 3).unwrap().red(), 3, 1), "未上传的纹理 id 不应绘制");
}

#[test]
fn texture_uv_out_of_bounds_is_clamped() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(10, 10, [0.0, 0.0, 0.0, 255.0]);
    f.uploads.push(TexUpload { id: 1, key: 1, w: 4, h: 4, data: solid_tex_data(4, 4, [255, 255, 0, 255]), premul: true });
    // UV 窗口越过源边界：[6,6,20,20] 应被 clamp 到源内有效区域，不得 panic。
    f.tex([0.0, 0.0, 6.0, 6.0], [0.0; 4], 1, [6.0, 6.0, 20.0, 20.0], 0.0, 1.0);
    let pm = b.render(&f);
    assert!(pm.pixel(2, 2).unwrap().red() > 200, "越界 UV clamp 后仍应取到有效黄色: {:?}", pm.pixel(2, 2).unwrap());
}

#[test]
fn texture_rounded_corner_mask_zeroes_corners() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(16, 16, [0.0, 0.0, 0.0, 255.0]);
    f.uploads.push(TexUpload { id: 1, key: 1, w: 16, h: 16, data: solid_tex_data(16, 16, [255, 255, 255, 255]), premul: true });
    f.tex([0.0, 0.0, 16.0, 16.0], [0.0; 4], 1, [0.0, 0.0, 16.0, 16.0], 8.0, 1.0);
    let pm = b.render(&f);
    assert_eq!(pm.pixel(0, 0).unwrap().red(), 0, "纹理圆角外应被遮罩清零");
    assert_eq!(pm.pixel(8, 8).unwrap().red(), 255, "纹理中心应保留");
}

#[test]
fn texture_alpha_scales_premultiplied_channels() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(4, 4, [0.0, 0.0, 0.0, 255.0]);
    f.uploads.push(TexUpload { id: 1, key: 1, w: 4, h: 4, data: solid_tex_data(4, 4, [255, 0, 0, 255]), premul: true });
    f.tex([0.0, 0.0, 4.0, 4.0], [0.0; 4], 1, [0.0, 0.0, 4.0, 4.0], 0.0, 0.5);
    let pm = b.render(&f);
    let p = pm.pixel(2, 2).unwrap();
    assert!(near(p.red(), 128, 20), "alpha=0.5 应把预乘红压到≈128: {:?}", p);
}

#[test]
fn texture_zero_alpha_is_invisible() {
    let mut b = CpuBackend::new();
    let mut f = GpuFrame::new(4, 4, [9.0, 9.0, 9.0, 255.0]);
    f.uploads.push(TexUpload { id: 1, key: 1, w: 4, h: 4, data: solid_tex_data(4, 4, [255, 0, 0, 255]), premul: true });
    f.tex([0.0, 0.0, 4.0, 4.0], [0.0; 4], 1, [0.0, 0.0, 4.0, 4.0], 0.0, 0.0);
    let pm = b.render(&f);
    assert!(near(pm.pixel(2, 2).unwrap().red(), 9, 1), "alpha=0 纹理不应可见");
}

// ---------------------------------------------------------------------------
// C ABI CPU 构建完整帧流程
// ---------------------------------------------------------------------------

fn cquad_solid() -> CQuad {
    CQuad {
        rect: [0.0, 0.0, 10.0, 10.0], clip: [0.0; 4], color: [255.0, 0.0, 0.0, 255.0],
        radius: 0.0, border: 0.0, dir: [0.0, 0.0], stops: [0.0; 40], stop_count: 0,
    }
}

#[test]
fn capi_cpu_device_creation_and_backend_kind() {
    let dev = gx_gpu_new(10, 10, 0, 0);
    assert!(!dev.is_null(), "CPU 构建无需窗口句柄，应可创建设备");
    assert_eq!(gx_gpu_backend_kind(dev), GX_GPU_KIND_CPU, "CPU 构建必须上报 2");
    unsafe { gx_gpu_free(dev) };
}

#[test]
fn capi_full_frame_roundtrip_reads_rgba() {
    let dev = gx_gpu_new(10, 10, 0, 0);
    assert!(!dev.is_null());
    let clear = [10.0f32, 20.0, 30.0, 255.0];
    unsafe {
        gx_gpu_begin(dev, 10, 10, clear.as_ptr());
        let mut q = cquad_solid();
        q.rect = [2.0, 2.0, 6.0, 6.0]; // 局部 quad，留角落作清屏色对照
        gx_gpu_quad(dev, &q);
        assert_eq!(gx_gpu_end(dev), 0, "CPU 构建 end 应返回成功");
        let mut buf = vec![0u8; 10 * 10 * 4];
        assert_eq!(gx_gpu_read_rgba(dev, buf.as_mut_ptr(), buf.len() as i32), 0);
        // 中心是 quad 的红色，角落是清屏色。
        let c = (5 * 10 + 5) * 4;
        assert!(near(buf[c], 255, 2), "中心应是红色, got {}", buf[c]);
        let corner = 0;
        assert!(near(buf[corner], 10, 2), "角落应是清屏色, got {}", buf[corner]);
        gx_gpu_free(dev);
    }
}

#[test]
fn capi_read_rgba_with_insufficient_buffer_returns_negative() {
    let dev = gx_gpu_new(10, 10, 0, 0);
    unsafe {
        gx_gpu_begin(dev, 10, 10, std::ptr::null());
        assert_eq!(gx_gpu_end(dev), 0);
        let mut small = [0u8; 16];
        assert_eq!(gx_gpu_read_rgba(dev, small.as_mut_ptr(), 16), -1, "缓冲不足应返回 -1");
        gx_gpu_free(dev);
    }
}

#[test]
fn capi_read_rgba_before_any_end_returns_no_frame() {
    let dev = gx_gpu_new(4, 4, 0, 0);
    unsafe {
        // 尚未 end，无最近帧可回读。
        let mut buf = [0u8; 4 * 4 * 4];
        assert_eq!(gx_gpu_read_rgba(dev, buf.as_mut_ptr(), buf.len() as i32), 3);
        gx_gpu_free(dev);
    }
}

#[test]
fn capi_upload_short_buffer_is_rejected() {
    let dev = gx_gpu_new(8, 8, 0, 0);
    unsafe {
        gx_gpu_begin(dev, 8, 8, std::ptr::null());
        let data = [255u8; 4]; // 远小于 8*8*4
        gx_gpu_upload_rgba(dev, 1, 1, 8, 8, data.as_ptr(), 4, 1);
        let t = CTex { rect: [0.0, 0.0, 8.0, 8.0], clip: [0.0; 4], id: 1, uv: [0.0, 0.0, 8.0, 8.0], radius: 0.0, alpha: 1.0 };
        gx_gpu_tex(dev, &t);
        assert_eq!(gx_gpu_end(dev), 0);
        let mut buf = vec![0u8; 8 * 8 * 4];
        gx_gpu_read_rgba(dev, buf.as_mut_ptr(), buf.len() as i32);
        assert_eq!(buf[0], 0, "数据不足的纹理不应上传，更不应绘制");
        gx_gpu_free(dev);
    }
}

#[test]
fn capi_quad_grad_stops_beyond_eight_are_truncated() {
    let dev = gx_gpu_new(4, 4, 0, 0);
    unsafe {
        gx_gpu_begin(dev, 4, 4, std::ptr::null());
        let mut q = cquad_solid();
        q.stop_count = 100; // 超过 MAX_STOPS，C ABI 应 min(8) 截断，不得读越界
        q.stops = [0.0; 40];
        gx_gpu_quad(dev, &q);
        assert_eq!(gx_gpu_end(dev), 0, "stop_count 超限不得崩溃");
        gx_gpu_free(dev);
    }
}

#[test]
fn capi_resize_changes_frame_dimension() {
    let dev = gx_gpu_new(2, 2, 0, 0);
    unsafe {
        gx_gpu_resize(dev, 4, 4);
        // resize 改的是 frame 尺寸；begin 会按传入尺寸重建，这里 end 后回读应为 resize 尺寸。
        assert_eq!(gx_gpu_end(dev), 0);
        let mut buf = [0u8; 4 * 4 * 4];
        assert_eq!(gx_gpu_read_rgba(dev, buf.as_mut_ptr(), buf.len() as i32), 0,
            "resize(4,4) 后帧应为 4x4=64 字节");
        gx_gpu_free(dev);
    }
}

#[test]
fn capi_upload_persists_across_begin_and_draws() {
    let dev = gx_gpu_new(4, 4, 0, 0);
    unsafe {
        let data = [255u8; 4 * 4 * 4];
        gx_gpu_upload_rgba(dev, 3, 77, 4, 4, data.as_ptr(), data.len() as i32, 1);
        // 跨帧 begin 应保留 uploads，随后 tex 仍能绘制。
        gx_gpu_begin(dev, 4, 4, std::ptr::null());
        let t = CTex { rect: [0.0, 0.0, 4.0, 4.0], clip: [0.0; 4], id: 3, uv: [0.0, 0.0, 4.0, 4.0], radius: 0.0, alpha: 1.0 };
        gx_gpu_tex(dev, &t);
        assert_eq!(gx_gpu_end(dev), 0);
        let mut buf = [0u8; 4 * 4 * 4];
        gx_gpu_read_rgba(dev, buf.as_mut_ptr(), buf.len() as i32);
        assert_eq!(buf[0], 255, "跨 begin 保留的纹理应可绘制");
        gx_gpu_free(dev);
    }
}
