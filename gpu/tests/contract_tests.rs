//! gx_gpu 公共契约测试（GPU / CPU 两种 feature 构建下都运行）。
//!
//! 这里固化的是「与后端无关的语义锚点」：GpuFrame 数据模型、渐变数学、
//! CSS 角度口径、C ABI 结构体布局与空指针安全。它们是两个后端共享的
//! 契约——任一构建下失败，都意味着 API 形状漂移，Go CGO / Android NDK /
//! Rust 后端至少有一方会读到错误数据。
//!
//! 像素级光栅行为见 cpu_raster_tests.rs（仅 cpu feature）。

use gx_gpu::capi::{
    gx_gpu_backend_kind, gx_gpu_begin, gx_gpu_end, gx_gpu_free, gx_gpu_new, gx_gpu_quad,
    gx_gpu_read_rgba, gx_gpu_resize, gx_gpu_tex, gx_gpu_upload_rgba, CQuad, CTex,
    GX_GPU_KIND_CPU, GX_GPU_KIND_GPU,
};
use gx_gpu::{
    clip_of, css_angle_to_dir, gradient_line_len, GpuFrame, Prim, TexUpload, MAX_GRADIENTS,
    MAX_STOPS,
};
use std::sync::Arc;

const EPS: f32 = 1e-5;

fn approx(a: f32) -> bool {
    a.abs() < EPS
}

// ---------------------------------------------------------------------------
// A. GpuFrame 数据模型
// ---------------------------------------------------------------------------

#[test]
fn frame_new_initializes_every_field() {
    let f = GpuFrame::new(800, 600, [1.0, 2.0, 3.0, 255.0]);
    assert_eq!(f.w, 800, "帧宽应原样写入");
    assert_eq!(f.h, 600, "帧高应原样写入");
    assert_eq!(f.clear, [1.0, 2.0, 3.0, 255.0], "清屏色应逐分量写入");
    assert!(f.uploads.is_empty(), "新帧不应含纹理上传");
    assert!(f.prims.is_empty(), "新帧不应含图元");
}

#[test]
fn frame_default_is_zero_sized_opaque_black() {
    let f = GpuFrame::default();
    assert_eq!((f.w, f.h), (0, 0));
    assert_eq!(f.clear, [0.0; 4]);
}

#[test]
fn quad_appends_single_solid_prim_in_order() {
    let mut f = GpuFrame::new(10, 10, [0.0; 4]);
    f.quad([1.0, 2.0, 3.0, 4.0], [0.0, 0.0, 5.0, 5.0], [9.0, 8.0, 7.0, 6.0], 2.5, 0.0);
    f.quad([5.0, 6.0, 7.0, 8.0], [0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0, 255.0], 0.0, 1.0);
    assert_eq!(f.prims.len(), 2, "两次 quad 应追加两条图元");
    match &f.prims[0] {
        Prim::Quad(q) => {
            assert_eq!(q.rect, [1.0, 2.0, 3.0, 4.0]);
            assert_eq!(q.clip, [0.0, 0.0, 5.0, 5.0]);
            assert_eq!(q.color, [9.0, 8.0, 7.0, 6.0]);
            assert!(approx(q.radius - 2.5));
            assert!(approx(q.border - 0.0));
            assert!(q.grad.is_none(), "quad() 走纯色路径，不应带渐变");
        }
        _ => panic!("quad() 必须产生 Prim::Quad"),
    }
    match &f.prims[1] {
        Prim::Quad(q) => assert!(approx(q.border - 1.0), "第二条应是描边 quad"),
        _ => panic!("quad() 必须产生 Prim::Quad"),
    }
}

#[test]
fn gradient_appends_white_base_with_grad() {
    let mut f = GpuFrame::new(10, 10, [0.0; 4]);
    f.gradient(
        [0.0, 0.0, 10.0, 20.0],
        [1.0, 1.0, 8.0, 8.0],
        3.0,
        gx_gpu::Grad {
            dx: 1.0,
            dy: 0.0,
            stops: vec![[0.0, 255.0, 0.0, 0.0, 255.0], [1.0, 0.0, 0.0, 255.0, 255.0]],
        },
    );
    assert_eq!(f.prims.len(), 1);
    match &f.prims[0] {
        Prim::Quad(q) => {
            assert_eq!(q.color, [255.0, 255.0, 255.0, 255.0], "渐变基色必须是白色");
            assert!(approx(q.border - 0.0), "渐变不应带描边");
            assert!(approx(q.radius - 3.0));
            let g = q.grad.as_ref().expect("gradient() 必须带 Grad");
            assert!(approx(g.dx - 1.0) && approx(g.dy), "渐变方向应原样写入");
            assert_eq!(g.stops.len(), 2);
        }
        _ => panic!("gradient() 必须产生 Prim::Quad"),
    }
}

#[test]
fn tex_appends_all_fields() {
    let mut f = GpuFrame::new(16, 16, [0.0; 4]);
    f.tex([1.0, 2.0, 8.0, 9.0], [0.0, 0.0, 16.0, 16.0], 42, [0.0, 2.0, 8.0, 10.0], 4.0, 0.25);
    match &f.prims[0] {
        Prim::Tex(t) => {
            assert_eq!(t.rect, [1.0, 2.0, 8.0, 9.0]);
            assert_eq!(t.clip, [0.0, 0.0, 16.0, 16.0]);
            assert_eq!(t.id, 42);
            assert_eq!(t.uv, [0.0, 2.0, 8.0, 10.0]);
            assert!(approx(t.radius - 4.0));
            assert!(approx(t.alpha - 0.25));
        }
        _ => panic!("tex() 必须产生 Prim::Tex"),
    }
}

#[test]
fn prim_clip_dispatches_both_variants() {
    let mut f = GpuFrame::new(10, 10, [0.0; 4]);
    f.quad([0.0; 4], [3.0, 4.0, 5.0, 6.0], [0.0; 4], 0.0, 0.0);
    f.tex([0.0; 4], [7.0, 8.0, 9.0, 10.0], 1, [0.0; 4], 0.0, 1.0);
    assert_eq!(f.prims[0].clip(), [3.0, 4.0, 5.0, 6.0], "Quad::clip 应透传");
    assert_eq!(f.prims[1].clip(), [7.0, 8.0, 9.0, 10.0], "Tex::clip 应透传");
}

#[test]
fn tex_upload_carries_all_fields() {
    let data = Arc::new(vec![255u8; 16]);
    let up = TexUpload {
        id: 7,
        key: 0xDEAD_BEEF_CAFE_1234,
        w: 2,
        h: 2,
        data: data.clone(),
        premul: true,
    };
    assert_eq!(up.id, 7);
    assert_eq!(up.key, 0xDEAD_BEEF_CAFE_1234);
    assert_eq!((up.w, up.h), (2, 2));
    assert_eq!(up.data.len(), 16);
    assert!(up.premul);
    assert!(Arc::ptr_eq(&up.data, &data), "上传数据应使用 Arc 零拷贝共享");
}

// ---------------------------------------------------------------------------
// B. 裁剪转换
// ---------------------------------------------------------------------------

#[test]
fn clip_of_none_means_unclipped() {
    assert_eq!(clip_of(None), [0.0, 0.0, 0.0, 0.0], "None 必须编码为 w=0");
}

#[test]
fn clip_of_some_passthrough_values() {
    assert_eq!(clip_of(Some((0.0, 0.0, 100.0, 100.0))), [0.0, 0.0, 100.0, 100.0]);
    assert_eq!(clip_of(Some((-3.5, 2.25, 7.0, 9.5))), [-3.5, 2.25, 7.0, 9.5]);
}

#[test]
fn clip_of_zero_size_clip_is_preserved() {
    assert_eq!(clip_of(Some((1.0, 2.0, 0.0, 3.0))), [1.0, 2.0, 0.0, 3.0]);
}

// ---------------------------------------------------------------------------
// C. 渐变数学（与 Go/Skia 口径一致）
// ---------------------------------------------------------------------------

#[test]
fn css_angle_cardinals_match_documented_semantics() {
    // 文档口径：0° 向上=(0,-1)，90° 向右=(1,0)，180° 向下=(0,1)，270° 向左=(-1,0)。
    let cases: [(f32, f32, f32); 4] = [
        (0.0, 0.0, -1.0),
        (90.0, 1.0, 0.0),
        (180.0, 0.0, 1.0),
        (270.0, -1.0, 0.0),
    ];
    for (deg, ex, ey) in cases {
        let (dx, dy) = css_angle_to_dir(deg);
        assert!(approx(dx - ex) && approx(dy - ey), "CSS {deg}° 应得 ({ex},{ey})，实得 ({dx},{dy})");
    }
}

#[test]
fn css_angle_diagonals_are_normalized() {
    let k = std::f32::consts::FRAC_1_SQRT_2;
    for (deg, ex, ey) in [(45.0, k, -k), (135.0, k, k), (225.0, -k, k), (315.0, -k, -k)] {
        let (dx, dy) = css_angle_to_dir(deg);
        assert!(approx(dx - ex) && approx(dy - ey), "CSS {deg}° 方向错: ({dx},{dy})");
        assert!(approx(dx * dx + dy * dy - 1.0), "CSS {deg}° 必须是单位向量");
    }
}

#[test]
fn css_angle_wraps_and_negatives() {
    let (dx0, dy0) = css_angle_to_dir(0.0);
    let (dx360, dy360) = css_angle_to_dir(360.0);
    assert!(approx(dx0 - dx360) && approx(dy0 - dy360), "360° 应与 0° 同向");
    let (dxn, dyn_) = css_angle_to_dir(-90.0);
    let (dx270, dy270) = css_angle_to_dir(270.0);
    assert!(approx(dxn - dx270) && approx(dyn_ - dy270), "-90° 应与 270° 同向");
    let (dx450, dy450) = css_angle_to_dir(450.0);
    let (dx90, dy90) = css_angle_to_dir(90.0);
    assert!(approx(dx450 - dx90) && approx(dy450 - dy90), "450° 应与 90° 同向");
}

#[test]
fn gradient_line_len_axis_aligned() {
    assert!(approx(gradient_line_len(100.0, 50.0, 0.0, -1.0) - 25.0), "竖直渐变 len=高/2");
    assert!(approx(gradient_line_len(100.0, 50.0, 1.0, 0.0) - 50.0), "水平渐变 len=宽/2");
    assert!(approx(gradient_line_len(80.0, 60.0, -1.0, 0.0) - 40.0), "负方向取绝对值");
}

#[test]
fn gradient_line_len_diagonal_is_projection_sum() {
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let got = gradient_line_len(100.0, 100.0, k, k);
    assert!(approx(got - 100.0 * k), "对角 len=(w*|dx|+h*|dy|)/2={}, 实得 {got}", 100.0 * k);
}

#[test]
fn gradient_line_len_zero_size() {
    assert!(approx(gradient_line_len(0.0, 0.0, 1.0, 0.0)));
    assert!(approx(gradient_line_len(10.0, 0.0, 0.0, -1.0)));
}

#[test]
fn gradient_capacity_constants_are_stable() {
    assert_eq!(MAX_STOPS, 8, "uniform 表每渐变 8 槽——ABI 契约，改动须同步 WGSL");
    assert_eq!(MAX_GRADIENTS, 64, "每帧最多 64 渐变——ABI 契约");
}

// ---------------------------------------------------------------------------
// D. C ABI 布局常量
// ---------------------------------------------------------------------------

#[test]
fn backend_kind_constants_are_stable() {
    assert_eq!(GX_GPU_KIND_GPU, 1);
    assert_eq!(GX_GPU_KIND_CPU, 2);
    assert_ne!(GX_GPU_KIND_GPU, GX_GPU_KIND_CPU, "两种后端常量必须可区分");
}

#[test]
fn c_struct_sizes_match_header_contract() {
    // gx_gpu.h 与 Go CGO 按此偏移布局，任一变化必须三处同步。
    assert_eq!(std::mem::size_of::<CQuad>(), 57 * 4, "CQuad = 57 个 4 字节槽");
    assert_eq!(std::mem::size_of::<CTex>(), 15 * 4, "CTex = 15 个 4 字节槽");
    assert_eq!(std::mem::align_of::<CQuad>(), 4, "全 f32/i32 结构应对齐 4");
    assert_eq!(std::mem::align_of::<CTex>(), 4);
}

#[test]
fn cquad_default_literal_is_well_formed() {
    let q = CQuad {
        rect: [0.0, 0.0, 10.0, 10.0],
        clip: [0.0, 0.0, 0.0, 0.0],
        color: [255.0, 0.0, 0.0, 255.0],
        radius: 2.0,
        border: 0.0,
        dir: [0.0, -1.0],
        stops: [0.0; 40],
        stop_count: 0,
    };
    assert_eq!(q.rect[2], 10.0);
    assert_eq!(q.stop_count, 0);
}

// ---------------------------------------------------------------------------
// E. C ABI 空指针安全（两种 feature 都必须不崩溃）
// ---------------------------------------------------------------------------

#[test]
fn free_null_is_noop() {
    gx_gpu_free(std::ptr::null_mut());
}

#[test]
fn resize_null_is_noop() {
    gx_gpu_resize(std::ptr::null_mut(), 100, 100);
}

#[test]
fn begin_null_is_noop() {
    let clear = [0.0f32; 4];
    gx_gpu_begin(std::ptr::null_mut(), 10, 10, clear.as_ptr());
}

#[test]
fn quad_null_dev_and_null_ptr_are_noop() {
    let q = CQuad {
        rect: [0.0; 4], clip: [0.0; 4], color: [255.0; 4], radius: 0.0, border: 0.0,
        dir: [0.0, 0.0], stops: [0.0; 40], stop_count: 0,
    };
    gx_gpu_quad(std::ptr::null_mut(), &q); // dev 空
    // 有效 dev 也要容忍 quad 指针为空（下面用 cpu 构建覆盖；gpu 构建 dev 可能为 null）。
    gx_gpu_quad(std::ptr::null_mut(), std::ptr::null());
}

#[test]
fn tex_null_dev_and_null_ptr_are_noop() {
    let t = CTex {
        rect: [0.0; 4], clip: [0.0; 4], id: 1, uv: [0.0; 4], radius: 0.0, alpha: 1.0,
    };
    gx_gpu_tex(std::ptr::null_mut(), &t);
    gx_gpu_tex(std::ptr::null_mut(), std::ptr::null());
}

#[test]
fn upload_null_and_invalid_params_are_noop() {
    gx_gpu_upload_rgba(std::ptr::null_mut(), 1, 1, 2, 2, std::ptr::null(), 16, 1);
    let data = [255u8; 16];
    // dev 为空时无论数据如何都安全。
    gx_gpu_upload_rgba(std::ptr::null_mut(), 1, 1, 2, 2, data.as_ptr(), 16, 1);
}

#[test]
fn end_null_returns_error_sentinel() {
    assert_eq!(gx_gpu_end(std::ptr::null_mut()), 1, "空句柄 end 必须返回 1");
}

#[test]
fn read_rgba_null_returns_error_sentinel() {
    let mut buf = [0u8; 16];
    assert_eq!(gx_gpu_read_rgba(std::ptr::null_mut(), buf.as_mut_ptr(), 16), 1);
}

#[test]
fn backend_kind_on_null_is_deterministic() {
    // 常量返回不依赖句柄内容；两种 feature 下都返回各自的编译期常量。
    let kind = gx_gpu_backend_kind(std::ptr::null_mut());
    assert!(kind == GX_GPU_KIND_GPU || kind == GX_GPU_KIND_CPU || kind == 0,
        "backend_kind 只能返回 1/2/0，实得 {kind}");
}
