//! 样式解析与动画插值。语义与 Skia 后端 backend/skia/style.go 完全对齐：
//! host 只透传 Props 字符串，后端在此解释 position/overflow/transform/transition/shadow。
//! 所有解析失败静默回退默认值，后端永不因脏 props 崩溃。

use crate::scene::SceneNode;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Position {
    Relative,
    Absolute,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Overflow {
    Visible,
    Hidden,
    Scroll,
    Auto,
}

/// 缓动函数。四档关键字走 Web 标准 cubic-bezier 曲线；自定义 cubic-bezier(x1,y1,x2,y2)
/// 与 CSS 语义一致（控制点 x 限 0..=1，y 不限，与浏览器一致）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicBezier(f32, f32, f32, f32),
}

impl Easing {
    /// 展开为 cubic-bezier 的四个控制点（CSS 标准值）。
    fn points(self) -> (f32, f32, f32, f32) {
        match self {
            Easing::Linear => (0.0, 0.0, 1.0, 1.0),
            Easing::Ease => (0.25, 0.1, 0.25, 1.0),
            Easing::EaseIn => (0.42, 0.0, 1.0, 1.0),
            Easing::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Easing::EaseInOut => (0.42, 0.0, 0.58, 1.0),
            Easing::CubicBezier(x1, y1, x2, y2) => (x1, y1, x2, y2),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BoxShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: (u8, u8, u8, u8), // ARGB alpha
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale: f32,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            translate_x: 0.0,
            translate_y: 0.0,
            scale: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TransitionItem {
    pub property: String,
    pub duration: f32, // ms
    pub delay: f32,    // ms
    pub easing: Easing,
}

/// 一帧解析出的样式。只保留与新增能力相关的字段；flex 仍由 Taffy 直接读 props。
#[derive(Clone, Debug)]
pub struct Style {
    pub opacity: f32,
    pub position: Position,
    pub inset: [f32; 4], // top,right,bottom,left；NaN = auto
    pub overflow: Overflow,
    pub scroll_top: f32,
    pub scroll_left: f32,
    pub transform: Transform,
    pub shadow: Option<BoxShadow>,
    pub transition: Vec<TransitionItem>,
    /// 绘制层级（z-index）：不影响 Taffy 布局，emit 时按值稳定排序绘制/命中顺序。
    pub z_index: i32,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            position: Position::Relative,
            inset: [f32::NAN; 4],
            overflow: Overflow::Visible,
            scroll_top: 0.0,
            scroll_left: 0.0,
            transform: Transform::default(),
            shadow: None,
            transition: vec![],
            z_index: 0,
        }
    }
}

/// 声明式动画解析结果，语义与 Go 端 gxscene.AnimationSpec 严格对齐。
#[derive(Clone, Copy, Debug)]
pub struct AnimationSpec {
    pub duration_ms: i64,
    pub iterations: i64, // -1 = infinite
}

impl AnimationSpec {
    /// 按绝对时钟 now_ms 采样 blink 的 opacity。返回 None 表示不覆盖基础 opacity。
    /// 相位 < 50% → 1.0，否则 0.0；有限次数结束后不覆盖（fill-mode:none）。
    pub fn sample_blink(&self, now_ms: i64) -> Option<f32> {
        if self.duration_ms <= 0 {
            return None;
        }
        if self.iterations > 0 && now_ms >= self.duration_ms * self.iterations {
            return None;
        }
        let phase = now_ms % self.duration_ms;
        Some(if phase * 2 < self.duration_ms {
            1.0
        } else {
            0.0
        })
    }
}

/// 解析 animation 标量（如 "blink 1.06s infinite"），只接受内置 blink 动画。
fn parse_duration_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("ms") {
        return v.trim().parse::<f64>().ok().map(|x| x as i64);
    }
    if let Some(v) = s.strip_suffix('s') {
        return v.trim().parse::<f64>().ok().map(|x| (x * 1000.0) as i64);
    }
    s.parse::<f64>().ok().map(|x| (x * 1000.0) as i64)
}

pub fn parse_animation(s: &str) -> Option<AnimationSpec> {
    let lowered = s.trim().to_ascii_lowercase();
    let toks: Vec<&str> = lowered.split_whitespace().collect();
    if toks.is_empty() || toks[0] != "blink" {
        return None;
    }
    let mut spec = AnimationSpec {
        duration_ms: 0,
        iterations: -1,
    };
    for t in &toks[1..] {
        if *t == "infinite" {
            spec.iterations = -1;
            continue;
        }
        if let Ok(n) = t.parse::<i64>() {
            if n > 0 {
                spec.iterations = n;
                continue;
            }
        }
        if let Some(d) = parse_duration_ms(t) {
            if d > 0 {
                spec.duration_ms = d;
            }
        }
    }
    if spec.duration_ms > 0 {
        Some(spec)
    } else {
        None
    }
}

/// 从节点 props 读取 animation 并解析。
pub fn animation_from_props(n: &SceneNode) -> Option<AnimationSpec> {
    n.props.get("animation").and_then(|s| parse_animation(s))
}

/// 扫描整棵树是否存在活跃的声明式动画。用于渲染前置判断：周期动画无外部事件
/// 唤醒，即使 transition 静止也必须进入全量重建路径逐帧采样。
pub fn tree_has_animation(n: &SceneNode) -> bool {
    if animation_from_props(n).is_some() {
        return true;
    }
    n.children.iter().any(tree_has_animation)
}

pub fn parse_px(s: &str) -> Option<f32> {
    s.trim().trim_end_matches("px").trim().parse::<f32>().ok()
}

/// CSS 1~4 值简写解析（padding/margin 通用）：1 值=四边，2 值=上下/左右，
/// 3 值=上/左右/下，4 值=上右下左。返回 [top, right, bottom, left]。
/// 任一分量非法则整体返回 None（与 Go 端 cssbox.Float32s 语义一致）。
pub fn parse_edges(s: &str) -> Option<[f32; 4]> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .map(|t| t.trim().trim_end_matches("px").trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    match parts.len() {
        1 => {
            let v = parts[0];
            Some([v, v, v, v])
        }
        2 => Some([parts[0], parts[1], parts[0], parts[1]]),
        3 => Some([parts[0], parts[1], parts[2], parts[1]]),
        4 => Some([parts[0], parts[1], parts[2], parts[3]]),
        _ => None,
    }
}

/// CSS white-space:normal/nowrap 折叠语义：连续空白折叠成单个空格并去首尾。
/// 默认（未声明/pre/pre-wrap）保持逐字节渲染——编辑器代码行缩进依赖原样空格。
pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 报告 white-space 值是否启用空白折叠。
pub fn white_space_collapses(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "normal" | "nowrap")
}

/// 从 props 解析字重：font-weight（CSS 标准）优先，weight（仓库历史约定）兜底。
/// "bold"/"bolder" → 700；数值 100..900 原样（clamp 到合法区间）。未声明返回 None。
pub fn font_weight_from_props(props: &std::collections::HashMap<String, String>) -> Option<u32> {
    let raw = props.get("font-weight").or_else(|| props.get("weight"))?;
    let t = raw.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    if t == "bold" || t == "bolder" {
        return Some(700);
    }
    if t == "normal" || t == "lighter" {
        return Some(400);
    }
    let v: f32 = t.trim_end_matches("px").parse().ok()?;
    Some(v.clamp(100.0, 900.0) as u32)
}

/// text-align 值 → 0=left/start（默认）、1=center、2=right/end。
pub fn text_align_of(s: &str) -> u8 {
    match s.trim().to_ascii_lowercase().as_str() {
        "center" => 1,
        "right" | "end" => 2,
        _ => 0,
    }
}

/// line-height 解析：无单位数值（<4，如 1.5）按字号倍数；带 px 的数值（>=4）
/// 按绝对像素。返回 (倍数, 像素)，未设置/非法返回 (0.0, 0.0)。
pub fn line_height_of(s: &str) -> (f32, f32) {
    let Some(v) = parse_px(s) else {
        return (0.0, 0.0);
    };
    if v <= 0.0 {
        return (0.0, 0.0);
    }
    if v < 4.0 {
        (v, 0.0)
    } else {
        (0.0, v)
    }
}

/// CSS text-align 落笔偏移：内容宽 tw 小于盒宽 w 时 center/right 生效。
pub fn align_offset(align: u8, w: f32, tw: f32) -> f32 {
    if tw <= 0.0 || tw >= w {
        return 0.0;
    }
    match align {
        1 => (w - tw) / 2.0,
        2 => w - tw,
        _ => 0.0,
    }
}

fn prop<'a>(n: &'a SceneNode, k: &str) -> Option<&'a str> {
    n.props.get(k).map(|s| s.as_str())
}

fn num(v: &str) -> Option<f32> {
    parse_px(v)
}

pub fn parse_style(n: &SceneNode) -> Style {
    let mut s = Style::default();
    if let Some(v) = prop(n, "opacity").and_then(num) {
        s.opacity = v.clamp(0.0, 1.0);
    }
    if let Some(v) = prop(n, "position") {
        if v.trim().eq_ignore_ascii_case("absolute") {
            s.position = Position::Absolute;
        }
    }
    for (i, k) in ["top", "right", "bottom", "left"].iter().enumerate() {
        if let Some(v) = prop(n, k) {
            if v.trim().eq_ignore_ascii_case("auto") {
                s.inset[i] = f32::NAN;
            } else if let Some(x) = num(v) {
                s.inset[i] = x;
            }
        }
    }
    if let Some(v) = prop(n, "overflow") {
        s.overflow = match v.trim().to_ascii_lowercase().as_str() {
            "hidden" => Overflow::Hidden,
            "scroll" => Overflow::Scroll,
            "auto" => Overflow::Auto,
            _ => Overflow::Visible,
        };
    }
    if let Some(v) = prop(n, "scroll-top").and_then(num) {
        s.scroll_top = v;
    }
    if let Some(v) = prop(n, "scroll-left").and_then(num) {
        s.scroll_left = v;
    }
    if let Some(v) = prop(n, "transform") {
        s.transform = parse_transform(v);
    }
    if let Some(v) = prop(n, "box-shadow") {
        if !v.trim().is_empty() && !v.trim().eq_ignore_ascii_case("none") {
            s.shadow = parse_box_shadow(v);
        }
    }
    if let Some(v) = prop(n, "transition") {
        s.transition = parse_transition(v);
    }
    if let Some(v) = prop(n, "z-index").and_then(num) {
        s.z_index = v as i32;
    }
    s
}

pub fn parse_transform(s: &str) -> Transform {
    let mut t = Transform::default();
    let low = s.to_ascii_lowercase();
    if let Some(i) = low.find("translate(") {
        if let Some(j) = s[i..].find(')') {
            let inner = &s[i + "translate(".len()..i + j];
            let parts: Vec<&str> = inner.split(',').collect();
            if let Some(v) = parts.get(0).and_then(|x| num(x)) {
                t.translate_x = v;
            }
            if let Some(v) = parts.get(1).and_then(|x| num(x)) {
                t.translate_y = v;
            }
        }
    }
    if let Some(i) = low.find("translatex(") {
        if let Some(j) = s[i..].find(')') {
            if let Some(v) = num(&s[i + "translatex(".len()..i + j]) {
                t.translate_x = v;
            }
        }
    }
    if let Some(i) = low.find("translatey(") {
        if let Some(j) = s[i..].find(')') {
            if let Some(v) = num(&s[i + "translatey(".len()..i + j]) {
                t.translate_y = v;
            }
        }
    }
    if let Some(i) = low.find("scale(") {
        if let Some(j) = s[i..].find(')') {
            if let Some(v) = num(&s[i + "scale(".len()..i + j]) {
                if v > 0.0 {
                    t.scale = v;
                }
            }
        }
    }
    t
}

/// 解析 "offsetX offsetY blur spread color"；spread/color 可省略。
pub fn parse_box_shadow(s: &str) -> Option<BoxShadow> {
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() < 2 {
        return None;
    }
    let mut nums = Vec::new();
    let mut color = (0u8, 0u8, 0u8, 90u8);
    for f in &fields {
        if let Some(c) = parse_color_rgba(f) {
            color = c;
            continue;
        }
        if let Some(v) = num(f) {
            nums.push(v);
        } else {
            break;
        }
    }
    if nums.len() < 2 {
        return None;
    }
    Some(BoxShadow {
        offset_x: nums[0],
        offset_y: nums[1],
        blur: *nums.get(2).unwrap_or(&0.0),
        spread: *nums.get(3).unwrap_or(&0.0),
        color,
    })
}

/// 解析颜色，返回 (r,g,b,a)。支持 #rgb/#rrggbb/#rrggbbaa/rgb()/rgba()/命名色。
pub fn parse_color_rgba(s: &str) -> Option<(u8, u8, u8, u8)> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(h) = s.strip_prefix('#') {
        let expand = |x: &str| -> String {
            if x.len() == 3 {
                x.chars().map(|c| format!("{}{}", c, c)).collect()
            } else {
                x.to_string()
            }
        };
        let h = expand(h);
        if h.len() == 6 {
            return Some((
                u8::from_str_radix(&h[0..2], 16).ok()?,
                u8::from_str_radix(&h[2..4], 16).ok()?,
                u8::from_str_radix(&h[4..6], 16).ok()?,
                255,
            ));
        }
        if h.len() == 8 {
            return Some((
                u8::from_str_radix(&h[0..2], 16).ok()?,
                u8::from_str_radix(&h[2..4], 16).ok()?,
                u8::from_str_radix(&h[4..6], 16).ok()?,
                u8::from_str_radix(&h[6..8], 16).ok()?,
            ));
        }
    }
    if s.starts_with("rgba(") && s.ends_with(')') {
        let p: Vec<&str> = s[5..s.len() - 1].split(',').collect();
        if p.len() == 4 {
            let r = p[0].trim().parse().ok()?;
            let g = p[1].trim().parse().ok()?;
            let b = p[2].trim().parse().ok()?;
            let a: f32 = p[3].trim().parse().ok()?;
            let av = if a > 1.0 { a as u8 } else { (a * 255.0) as u8 };
            return Some((r, g, b, av));
        }
    }
    if s.starts_with("rgb(") && s.ends_with(')') {
        let p: Vec<&str> = s[4..s.len() - 1].split(',').collect();
        if p.len() == 3 {
            return Some((
                p[0].trim().parse().ok()?,
                p[1].trim().parse().ok()?,
                p[2].trim().parse().ok()?,
                255,
            ));
        }
    }
    Some(match s.as_str() {
        "white" => (255, 255, 255, 255),
        "black" => (0, 0, 0, 255),
        "red" => (220, 40, 40, 255),
        "green" => (40, 180, 60, 255),
        "blue" => (40, 90, 220, 255),
        "gray" | "grey" => (128, 128, 128, 255),
        "yellow" => (230, 200, 40, 255),
        _ => return None,
    })
}

/// 解析时长 token（"300ms" / "0.4s" / "400" 无单位视为 ms）。失败返回 None。
fn parse_duration_token(t: &str) -> Option<f32> {
    if let Some(x) = t.strip_suffix("ms") {
        return x.parse::<f32>().ok();
    }
    if let Some(x) = t.strip_suffix('s') {
        return x.parse::<f32>().ok().map(|v| v * 1000.0);
    }
    t.parse::<f32>().ok()
}

/// 解析 "cubic-bezier(x1,y1,x2,y2)"：x1/x2 钳到 0..=1（CSS 规范要求，保证 x(s) 单调），
/// y1/y2 不钳（允许过冲）。非法返回 None。
fn parse_cubic_bezier(t: &str) -> Option<Easing> {
    let low = t.to_ascii_lowercase();
    let inner = low.strip_prefix("cubic-bezier(")?.strip_suffix(')')?;
    let p: Vec<&str> = inner.split(',').collect();
    if p.len() != 4 {
        return None;
    }
    let x1 = p[0].trim().parse::<f32>().ok()?;
    let y1 = p[1].trim().parse::<f32>().ok()?;
    let x2 = p[2].trim().parse::<f32>().ok()?;
    let y2 = p[3].trim().parse::<f32>().ok()?;
    if !x1.is_finite() || !y1.is_finite() || !x2.is_finite() || !y2.is_finite() {
        return None;
    }
    Some(Easing::CubicBezier(
        x1.clamp(0.0, 1.0),
        y1,
        x2.clamp(0.0, 1.0),
        y2,
    ))
}

/// 把 transition 声明按顶层逗号切段，跳过 cubic-bezier(...) 内部的逗号。
fn split_transition_segments(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ',' if depth == 0 => {
                out.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].to_string());
    out
}

/// 把一段 transition 声明切成 token，cubic-bezier(...) 整体作为一个 token
/// （按顶层空白切，括号内空白不切）。
fn transition_tokens(seg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in seg.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
                cur.push(c);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn parse_transition(s: &str) -> Vec<TransitionItem> {
    let mut out = Vec::new();
    for seg in split_transition_segments(s) {
        let seg = seg.trim();
        if seg.is_empty() || seg.eq_ignore_ascii_case("none") {
            continue;
        }
        let mut it = TransitionItem {
            property: String::new(),
            duration: 0.0,
            delay: 0.0,
            easing: Easing::Ease,
        };
        // CSS 简写中两个时长按出现顺序为 duration、delay。
        let mut durations_seen = 0;
        for tok in transition_tokens(seg) {
            let t = tok.to_ascii_lowercase();
            if let Some(cb) = parse_cubic_bezier(&t) {
                it.easing = cb;
            } else if let Some(v) = parse_duration_token(&t) {
                if durations_seen == 0 {
                    it.duration = v;
                } else if durations_seen == 1 {
                    it.delay = v;
                }
                durations_seen += 1;
            } else {
                match t.as_str() {
                    "linear" => it.easing = Easing::Linear,
                    "ease-in" => it.easing = Easing::EaseIn,
                    "ease-out" => it.easing = Easing::EaseOut,
                    "ease-in-out" => it.easing = Easing::EaseInOut,
                    "ease" => it.easing = Easing::Ease,
                    _ => {
                        if it.property.is_empty() {
                            it.property = t.clone();
                        }
                    }
                }
            }
        }
        if !it.property.is_empty() && it.duration > 0.0 {
            out.push(it);
        }
    }
    out
}

/// 三次贝塞尔在参数 s 处的取值：3(1-s)²s·p1 + 3(1-s)s²·p2 + s³。
/// p0=0、p3=1 固定（CSS cubic-bezier 曲线端点恒为 (0,0) 与 (1,1)）。
#[inline]
fn cubic_bezier_at(s: f32, p1: f32, p2: f32) -> f32 {
    let one_s = 1.0 - s;
    3.0 * one_s * one_s * s * p1 + 3.0 * one_s * s * s * p2 + s * s * s
}

/// 三次贝塞尔在参数 s 处的导数，用于牛顿迭代。
#[inline]
fn cubic_bezier_slope(s: f32, p1: f32, p2: f32) -> f32 {
    let one_s = 1.0 - s;
    3.0 * one_s * one_s * p1 + 6.0 * one_s * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

/// 求 Web 标准 cubic-bezier(x1,y1,x2,y2) 在进度 t（时间比 0..=1）下的输出。
///
/// 算法：解 x(s)=t 得参数 s，再取 y(s)。先用牛顿迭代（8 步，收敛快），
/// 导数退化时回退二分（16 步，稳定）。x1/x2 被 parse_transition 约束在 0..=1，
/// 保证 x(s) 单调、解唯一。端点直接返回，规避浮点误差。
pub fn easing_value(e: Easing, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let (x1, y1, x2, y2) = e.points();
    if x1 == y1 && x2 == y2 {
        return t; // linear 特例
    }
    // 牛顿迭代
    let mut s = t;
    let mut solved = false;
    for _ in 0..8 {
        let x = cubic_bezier_at(s, x1, x2) - t;
        if x.abs() < 1e-6 {
            solved = true;
            break;
        }
        let d = cubic_bezier_slope(s, x1, x2);
        if d.abs() < 1e-6 {
            break; // 退化，交给二分
        }
        s -= x / d;
        s = s.clamp(0.0, 1.0);
    }
    if !solved {
        // 二分兜底：x(s) 在 0..1 单调
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        s = t;
        for _ in 0..24 {
            let cx = cubic_bezier_at(s, x1, x2);
            if (cx - t).abs() < 1e-6 {
                break;
            }
            if cx < t {
                lo = s;
            } else {
                hi = s;
            }
            s = (lo + hi) * 0.5;
        }
    }
    cubic_bezier_at(s.clamp(0.0, 1.0), y1, y2)
}

// ---------- 伪状态样式覆盖（&:hover / &:active） ----------
//
// host 把样式表展平后，普通属性直出为 props，伪状态子表编码成
// props["&:hover"] = `{"bg":"#2563eb"}` 这样的 JSON 串。本节在布局/绘制前，
// 按当前命中的 hovered/active 节点把对应子表覆盖进一棵临时树副本，
// 与 Animator::apply 同构——不污染常驻镜像树，下一帧状态变了自动还原。

/// 把某个节点的伪状态子表覆盖进它的 props。伪状态 key 用 "&:"，兼容旧 "$:"。
/// 返回是否发生了覆盖（供调用方决定是否要重建 DisplayList）。
fn overlay_pseudo(props: &mut std::collections::HashMap<String, String>, pseudo: &str) -> bool {
    let val = props
        .get(pseudo)
        .or_else(|| props.get(&pseudo.replacen("&:", "$:", 1)))
        .cloned();
    let Some(json) = val else {
        return false;
    };
    let Ok(obj) = serde_json::from_str::<serde_json::Value>(&json) else {
        return false;
    };
    let Some(map) = obj.as_object() else {
        return false;
    };
    let mut changed = false;
    for (k, v) in map {
        let s = match v {
            serde_json::Value::String(x) => x.clone(),
            serde_json::Value::Number(x) => x.to_string(),
            serde_json::Value::Bool(x) => x.to_string(),
            _ => continue,
        };
        props.insert(k.clone(), s);
        changed = true;
    }
    changed
}

/// 判断节点是否禁用。props 由 host 原样透传，禁用标记兼容 bool 与字符串：
/// "true"/"1" 视为禁用；其余（含缺省、"false"/"0"）视为启用。
fn is_disabled(n: &SceneNode) -> bool {
    match n
        .props
        .get("disabled")
        .map(|s| s.trim().to_ascii_lowercase())
    {
        Some(v) => v == "true" || v == "1",
        None => false,
    }
}

/// 按当前交互状态生成一棵 props 被伪状态覆盖的临时树。
///
/// 覆盖顺序（后者覆盖前者同名属性）：
///   hover → focus → active → disabled
/// 即 active 强于 hover/focus，disabled 又强于三者（禁用态应盖过任何交互态）。
/// 与 Animator::apply 同构：只克隆命中节点的 props，不污染常驻镜像树。
///
/// - hovered / active / focused 为当前命中的节点 ID（None 表示无此状态）。
/// - disabled 是节点自身的持久属性（props["disabled"]），无需外部载体：
///   即便鼠标悬停或聚焦，禁用节点的 &:disabled 都应生效并覆盖交互态。
/// 未命中或节点无对应伪状态子表时返回 clone（语义等价，调用方统一处理）。
pub fn apply_pseudo_state(
    root: &SceneNode,
    hovered: Option<&str>,
    active: Option<&str>,
    focused: Option<&str>,
) -> SceneNode {
    fn walk(
        n: &SceneNode,
        hovered: Option<&str>,
        active: Option<&str>,
        focused: Option<&str>,
    ) -> SceneNode {
        let mut out = n.clone();
        // 交互态匹配：nid 优先（GRX3 整数身份）、id 字符串兜底（GRX1/测试）。
        let hit = |target: &Option<&str>| target.map(|t| n.matches(t)).unwrap_or(false);
        // 交互态：hover 先、focus 次、active 后，逐级覆盖同名属性。
        if hit(&hovered) {
            overlay_pseudo(&mut out.props, "&:hover");
        }
        if hit(&focused) {
            overlay_pseudo(&mut out.props, "&:focus");
        }
        if hit(&active) {
            overlay_pseudo(&mut out.props, "&:active");
        }
        // 禁用态优先级最高：盖过 hover/focus/active 的覆盖结果。
        if is_disabled(n) {
            overlay_pseudo(&mut out.props, "&:disabled");
        }
        for c in &mut out.children {
            let old = c.clone();
            *c = walk(&old, hovered, active, focused);
        }
        out
    }
    walk(root, hovered, active, focused)
}

// ---------- transition 插值引擎 ----------

/// 可插值属性的一类值。四类分别走不同的插值/序列化路径。
#[derive(Clone, Debug, PartialEq)]
enum AnimValue {
    Number(f32),
    /// (r,g,b,a)，逐通道线性插值（含 alpha）。
    Color((u8, u8, u8, u8)),
    /// transform 三个分量（translate_x, translate_y, scale）逐分量插值。
    Transform(Transform),
}

impl AnimValue {
    /// 按属性名与 props 解析目标值。返回 None 表示该属性不可动画/值非法。
    fn parse(property: &str, props: &HashMap<String, String>) -> Option<Self> {
        let raw = props.get(property)?.as_str();
        if is_color_prop(property) {
            return parse_color_rgba(raw).map(Self::Color);
        }
        if property.eq_ignore_ascii_case("transform") {
            return Some(Self::Transform(parse_transform(raw)));
        }
        parse_px(raw).map(Self::Number)
    }

    /// 线性插值（t 已经过缓动）。
    fn lerp(&self, to: &Self, t: f32) -> Self {
        match (self, to) {
            (Self::Number(a), Self::Number(b)) => Self::Number(a + (b - a) * t),
            (Self::Color((ar, ag, ab, aa)), Self::Color((br, bg, bb, ba))) => {
                let ch = |a: u8, b: u8| -> u8 {
                    let v = a as f32 + (b as f32 - a as f32) * t;
                    v.round().clamp(0.0, 255.0) as u8
                };
                Self::Color((ch(*ar, *br), ch(*ag, *bg), ch(*ab, *bb), ch(*aa, *ba)))
            }
            (Self::Transform(a), Self::Transform(b)) => Self::Transform(Transform {
                translate_x: a.translate_x + (b.translate_x - a.translate_x) * t,
                translate_y: a.translate_y + (b.translate_y - a.translate_y) * t,
                scale: a.scale + (b.scale - a.scale) * t,
            }),
            _ => self.clone(), // 类型不一致（不该发生）：保守保持起点
        }
    }

    /// 序列化回 props 字符串；下游 parse_style / parse_px / parse_color_rgba 能再解析。
    /// 颜色用 #rrggbbaa 十六进制——避免 rgba() 中 alpha=1 被误判为 0..1 浮点（→255）。
    fn to_prop(&self) -> String {
        match self {
            Self::Number(v) => format!("{:.3}", v),
            Self::Color((r, g, b, a)) => format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a),
            Self::Transform(t) => format!(
                "translate({:.3}px, {:.3}px) scale({:.3})",
                t.translate_x, t.translate_y, t.scale
            ),
        }
    }
}

/// 颜色属性集合（与 docs/STYLE.md §3 一致：color/bg/background/border）。
fn is_color_prop(property: &str) -> bool {
    matches!(
        property.to_ascii_lowercase().as_str(),
        "color" | "bg" | "background" | "border" | "border-color" | "shadow-color"
    )
}

/// `all` 展开时要枚举的可动画属性。只含 parse_style 真正消费、且可插值的属性，
/// 避免把 transition/事件等非样式键当成动画目标。
const ANIMATABLE_PROPS: &[&str] = &[
    "opacity",
    "width",
    "height",
    "font-size",
    "padding",
    "margin",
    "gap",
    "row-gap",
    "column-gap",
    "border-radius",
    "border-width",
    "top",
    "right",
    "bottom",
    "left",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
    "flex-grow",
    "flex-shrink",
    "z-index",
    "line-height",
    "letter-spacing",
    "scroll-top",
    "scroll-left",
    "color",
    "bg",
    "background",
    "border",
    "transform",
];

/// 正在进行的单属性动画。
#[derive(Clone, Debug)]
struct Anim {
    from: AnimValue,
    to: AnimValue,
    start_ms: u128,
    duration: f32,
    delay: f32,
    easing: Easing,
}

/// 每个节点每个属性的动画状态。key = (node_id, property)。
type AnimMap = HashMap<(String, String), Anim>;

/// 动画引擎：渲染时用「目标 props vs 上次目标」检测变化并推进插值，
/// 返回一棵 props 被覆盖成当前插值值的临时场景树供本帧布局/绘制。
/// 覆盖是浅拷贝（只克隆变化节点的 props），不污染常驻镜像树。
pub struct Animator {
    anims: AnimMap,
    targets: HashMap<(String, String), AnimValue>,
    now_ms: u128,
    /// 本帧 apply 看到的活跃声明式动画（周期动画无外部事件唤醒，据此维持重绘）。
    has_animation: bool,
}

impl Animator {
    pub fn new() -> Self {
        Self {
            anims: AnimMap::new(),
            targets: HashMap::new(),
            now_ms: 0,
            has_animation: false,
        }
    }

    pub fn set_now(&mut self, now_ms: u128) {
        self.now_ms = now_ms;
    }

    /// 是否仍有进行中的动画。无变化帧可据此决定是否需要继续重绘。
    pub fn is_active(&self) -> bool {
        !self.anims.is_empty()
    }

    /// 本帧是否采样到活跃的声明式动画（如 blink），供宿主持续重绘。
    pub fn has_active_animation(&self) -> bool {
        self.has_animation
    }

    /// 返回当前插值值（若动画已结束则返回 to 并移除动画）。
    /// delay 期间固定在 from；delay 结束后按 (elapsed-delay)/duration 推进。
    fn sample(&mut self, key: &(String, String)) -> Option<AnimValue> {
        let now = self.now_ms;
        let a = self.anims.get(key)?;
        let elapsed = now.saturating_sub(a.start_ms) as f32;
        let active = elapsed - a.delay;
        if active >= a.duration {
            let to = a.to.clone();
            self.anims.remove(key);
            Some(to)
        } else if active < 0.0 {
            Some(a.from.clone()) // 延迟阶段：停在起点
        } else {
            let t = easing_value(a.easing, active / a.duration);
            Some(a.from.lerp(&a.to, t))
        }
    }

    /// 对整棵树应用动画覆盖，返回本帧要用的树副本。
    pub fn apply(&mut self, root: &SceneNode) -> SceneNode {
        self.has_animation = false; // 每帧重置，记录本帧看到的周期动画
        let mut out = root.clone();
        self.apply_node(root, &mut out);
        out
    }

    /// 对单个 transition 项启动/推进动画。property 已经过 all 展开。
    fn animate_property(
        &mut self,
        src: &SceneNode,
        dst: &mut SceneNode,
        property: &str,
        tr: &TransitionItem,
    ) {
        let Some(target) = AnimValue::parse(property, &src.props) else {
            return;
        };
        let key = (src.id.clone(), property.to_string());
        let prev = self.targets.get(&key).cloned();
        if prev.as_ref() != Some(&target) {
            // 目标变化：从「当前插值值」起新动画，平滑衔接中途改向。
            let from = self.sample(&key).or(prev).unwrap_or_else(|| target.clone());
            self.anims.insert(
                key.clone(),
                Anim {
                    from,
                    to: target.clone(),
                    start_ms: self.now_ms,
                    duration: tr.duration,
                    delay: tr.delay,
                    easing: tr.easing,
                },
            );
            self.targets.insert(key.clone(), target);
        }
        if let Some(v) = self.sample(&key) {
            dst.props.insert(property.to_string(), v.to_prop());
        }
    }

    fn apply_node(&mut self, src: &SceneNode, dst: &mut SceneNode) {
        // 声明式 animation（周期动画）：按绝对时钟采样 opacity 写进临时树，优先级
        // 高于 transition（对齐 CSS animation > transition）。覆盖值随时间变化，
        // needs_continuous_redraw 据此维持重绘。
        if let Some(spec) = animation_from_props(src) {
            self.has_animation = true;
            if let Some(op) = spec.sample_blink(self.now_ms as i64) {
                dst.props.insert("opacity".into(), format!("{:.3}", op));
            }
        }
        // 收集该节点的 transition 声明。
        let trs = parse_transition(
            src.props
                .get("transition")
                .map(|s| s.as_str())
                .unwrap_or(""),
        );
        for tr in &trs {
            if tr.property == "all" {
                // all 展开为节点上实际存在且可动画的属性，各自套用同一时长/缓动。
                for &p in ANIMATABLE_PROPS {
                    if src.props.contains_key(p) {
                        self.animate_property(src, dst, p, tr);
                    }
                }
            } else {
                self.animate_property(src, dst, &tr.property, tr);
            }
        }
        for (i, c) in src.children.iter().enumerate() {
            self.apply_node(c, &mut dst.children[i]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binprotocol::NodeType;

    fn node(props: &[(&str, &str)]) -> SceneNode {
        let mut p = HashMap::new();
        for (k, v) in props {
            p.insert(k.to_string(), v.to_string());
        }
        SceneNode {
            typ: NodeType::Div,
            kind: "div".into(),
            id: "n1".into(),
            text: String::new(),
            nid: 1010,
            props: p,
            events: vec![],
            children: vec![],
        }
    }

    #[test]
    fn apply_pseudo_state_overlays_hover_focus_active() {
        let mut n = node(&[]);
        n.id = "btn".into();
        n.props.insert("bg".into(), "#fff".into());
        n.props
            .insert("&:hover".into(), r##"{"bg":"#eee","color":"#333"}"##.into());
        n.props
            .insert("&:focus".into(), r##"{"border":"#00f"}"##.into());
        n.props
            .insert("&:active".into(), r##"{"bg":"#ddd"}"##.into());
        let mut child = node(&[]);
        child.id = "label".into();
        child.props.insert("bg".into(), "#000".into());
        n.children.push(child);

        // 无命中：原样
        let plain = apply_pseudo_state(&n, None, None, None);
        assert_eq!(plain.props["bg"], "#fff");
        assert!(!plain.props.contains_key("color"));

        // hover：子表覆盖 bg + 新增 color
        let hov = apply_pseudo_state(&n, Some("btn"), None, None);
        assert_eq!(hov.props["bg"], "#eee");
        assert_eq!(hov.props["color"], "#333");
        assert_eq!(hov.children[0].props["bg"], "#000", "未命中子节点不受影响");

        // focus：只套 focus 子表
        let foc = apply_pseudo_state(&n, None, None, Some("btn"));
        assert_eq!(foc.props["bg"], "#fff", "focus 未声明 bg 应保留基础值");
        assert_eq!(foc.props["border"], "#00f", "focus 子表新增 border");

        // hover + focus + active：active 覆盖同名 bg，focus 的非冲突属性保留
        let act = apply_pseudo_state(&n, Some("btn"), Some("btn"), Some("btn"));
        assert_eq!(act.props["bg"], "#ddd");
        assert_eq!(act.props["color"], "#333", "hover 的非冲突属性保留");
        assert_eq!(act.props["border"], "#00f", "focus 的非冲突属性保留");

        // 原树不被污染
        assert_eq!(n.props["bg"], "#fff");
    }

    #[test]
    fn apply_pseudo_state_disabled_overrides_and_is_idempotent() {
        let mut n = node(&[]);
        n.id = "btn".into();
        n.props.insert("bg".into(), "#3b82f6".into());
        n.props.insert("disabled".into(), "true".into());
        n.props
            .insert("&:hover".into(), r##"{"bg":"#2563eb"}"##.into());
        n.props
            .insert("&:active".into(), r##"{"bg":"#1d4ed8"}"##.into());
        n.props
            .insert("&:focus".into(), r##"{"bg":"#60a5fa"}"##.into());
        n.props.insert(
            "&:disabled".into(),
            r##"{"bg":"#9ca3af","opacity":"0.6"}"##.into(),
        );

        // 禁用是节点自身属性，无外部载体也应生效，且盖过任何交互态。
        let dis = apply_pseudo_state(&n, Some("btn"), Some("btn"), Some("btn"));
        assert_eq!(
            dis.props["bg"], "#9ca3af",
            "disabled 应盖过 hover/focus/active"
        );
        assert_eq!(dis.props["opacity"], "0.6", "disabled 子表新增属性");

        // 字符串 "1" 同样识别为禁用。
        n.props.insert("disabled".into(), "1".into());
        let dis1 = apply_pseudo_state(&n, None, None, None);
        assert_eq!(dis1.props["bg"], "#9ca3af");

        // "false" / 缺省不禁用：交互态正常生效。
        n.props.insert("disabled".into(), "false".into());
        let en = apply_pseudo_state(&n, Some("btn"), None, None);
        assert_eq!(en.props["bg"], "#2563eb", "启用时 hover 正常生效");

        n.props.remove("disabled");
        let en2 = apply_pseudo_state(&n, None, None, None);
        assert_eq!(en2.props["bg"], "#3b82f6", "无 disabled 键时为基础值");
    }

    #[test]
    fn parse_position_inset_and_overflow() {
        let n = node(&[
            ("position", "absolute"),
            ("top", "10"),
            ("left", "20px"),
            ("right", "auto"),
            ("overflow", "scroll"),
            ("scroll-top", "42"),
        ]);
        let s = parse_style(&n);
        assert_eq!(s.position, Position::Absolute);
        assert_eq!(s.inset[0], 10.0);
        assert_eq!(s.inset[3], 20.0);
        assert!(s.inset[1].is_nan());
        assert_eq!(s.overflow, Overflow::Scroll);
        assert_eq!(s.scroll_top, 42.0);
    }

    #[test]
    fn parse_transform_shadow_transition() {
        let n = node(&[
            ("transform", "translate(10px, -4px) scale(1.5)"),
            ("box-shadow", "0 4px 12px 2px rgba(0,0,0,0.35)"),
            ("transition", "opacity 300ms ease-in-out"),
        ]);
        let s = parse_style(&n);
        assert_eq!(s.transform.translate_x, 10.0);
        assert_eq!(s.transform.translate_y, -4.0);
        assert_eq!(s.transform.scale, 1.5);
        let sh = s.shadow.unwrap();
        assert_eq!(sh.offset_y, 4.0);
        assert_eq!(sh.blur, 12.0);
        assert_eq!(sh.spread, 2.0);
        assert_eq!(s.transition.len(), 1);
        assert_eq!(s.transition[0].duration, 300.0);
        assert_eq!(s.transition[0].easing, Easing::EaseInOut);
    }

    #[test]
    fn easing_curve_matches_web_standard() {
        // 标准 CSS cubic-bezier 曲线，数值由浏览器同款求解器算出。
        // linear
        assert!((easing_value(Easing::Linear, 0.25) - 0.25).abs() < 0.001);
        assert!((easing_value(Easing::Linear, 0.5) - 0.5).abs() < 0.001);
        // ease = cubic-bezier(.25,.1,.25,1)
        assert!((easing_value(Easing::Ease, 0.25) - 0.40851).abs() < 0.005);
        assert!((easing_value(Easing::Ease, 0.5) - 0.80240).abs() < 0.005);
        assert!((easing_value(Easing::Ease, 0.75) - 0.96046).abs() < 0.005);
        // ease-in = cubic-bezier(.42,0,1,1)
        assert!((easing_value(Easing::EaseIn, 0.25) - 0.09346).abs() < 0.005);
        assert!((easing_value(Easing::EaseIn, 0.5) - 0.31536).abs() < 0.005);
        assert!((easing_value(Easing::EaseIn, 0.75) - 0.62186).abs() < 0.005);
        // ease-out = cubic-bezier(0,0,.58,1)
        assert!((easing_value(Easing::EaseOut, 0.25) - 0.37814).abs() < 0.005);
        assert!((easing_value(Easing::EaseOut, 0.5) - 0.68464).abs() < 0.005);
        assert!((easing_value(Easing::EaseOut, 0.75) - 0.90654).abs() < 0.005);
        // ease-in-out = cubic-bezier(.42,0,.58,1)（对称）
        assert!((easing_value(Easing::EaseInOut, 0.25) - 0.12916).abs() < 0.005);
        assert!((easing_value(Easing::EaseInOut, 0.5) - 0.5).abs() < 0.005);
        assert!((easing_value(Easing::EaseInOut, 0.75) - 0.87084).abs() < 0.005);
        // 端点
        assert_eq!(easing_value(Easing::Ease, 0.0), 0.0);
        assert_eq!(easing_value(Easing::Ease, 1.0), 1.0);
        // 超出区间钳制
        assert_eq!(easing_value(Easing::Ease, -0.5), 0.0);
        assert_eq!(easing_value(Easing::Ease, 1.5), 1.0);
    }

    #[test]
    fn easing_custom_cubic_bezier() {
        // 自定义曲线与 Web cubic-bezier() 一致；x 越界钳到 0..1（与浏览器一致）。
        let ease_in = Easing::CubicBezier(0.42, 0.0, 1.0, 1.0);
        assert!((easing_value(ease_in, 0.5) - 0.31536).abs() < 0.005);
        // y 允许过冲（back）
        let back = Easing::CubicBezier(0.34, 1.56, 0.64, 1.0);
        assert!(easing_value(back, 0.5) > 1.0, "back 曲线中段应过冲 >1");
        assert_eq!(easing_value(back, 1.0), 1.0, "端点仍为 1");
        // 直接构造越界的 x 控制点也应稳定、不 panic、输出有限。
        let clamped = Easing::CubicBezier(-1.0, 0.0, 2.0, 1.0);
        for i in 0..=10 {
            let v = easing_value(clamped, i as f32 / 10.0);
            assert!(v.is_finite(), "越界控制点输出应有限");
        }
    }

    #[test]
    fn parse_transition_cubic_bezier_and_delay() {
        let items = parse_transition(
            "opacity 300ms cubic-bezier(0.4, 0, 0.2, 1) 100ms, transform 0.2s ease-in 50ms",
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].property, "opacity");
        assert_eq!(items[0].duration, 300.0);
        assert_eq!(items[0].delay, 100.0);
        assert_eq!(items[0].easing, Easing::CubicBezier(0.4, 0.0, 0.2, 1.0));
        assert_eq!(items[1].property, "transform");
        assert_eq!(items[1].duration, 200.0);
        assert_eq!(items[1].delay, 50.0);
        assert_eq!(items[1].easing, Easing::EaseIn);
    }

    #[test]
    fn parse_transition_cubic_bezier_ignores_internal_commas() {
        let items = parse_transition("width 1s cubic-bezier(0.1, 0.9, 0.2, 1)");
        assert_eq!(items.len(), 1, "括号内逗号不应切出多条 transition");
        assert_eq!(items[0].easing, Easing::CubicBezier(0.1, 0.9, 0.2, 1.0));
    }

    #[test]
    fn parse_transition_invalid_cubic_bezier_falls_back() {
        // 非法曲线不 panic，回退到 default ease。
        let items = parse_transition("width 300ms cubic-bezier(0.1, 0.2)");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].easing, Easing::Ease);
    }

    #[test]
    fn animator_interpolates_numeric_prop() {
        let mut a = Animator::new();
        let mut n = node(&[("transition", "width 200ms linear"), ("width", "100")]);
        // 首帧：目标 100，无动画历史 → 直接 100
        a.set_now(0);
        let out = a.apply(&n);
        assert_eq!(out.props["width"], "100.000");
        // 目标改为 200 → 启动动画；t=0 → 起点 100
        n.props.insert("width".into(), "200".into());
        a.set_now(1000);
        let out = a.apply(&n);
        let v: f32 = out.props["width"].parse().unwrap();
        assert!((v - 100.0).abs() < 1.0, "t=0 应≈100, got {}", v);
        // t=100ms（中点）→ 150
        a.set_now(1100);
        let out = a.apply(&n);
        let v: f32 = out.props["width"].parse().unwrap();
        assert!((v - 150.0).abs() < 2.0, "中点应≈150, got {}", v);
        // t=200ms（结束）→ 200
        a.set_now(1200);
        let out = a.apply(&n);
        let v: f32 = out.props["width"].parse().unwrap();
        assert!((v - 200.0).abs() < 0.1, "结束应=200, got {}", v);
    }

    #[test]
    fn animator_is_active_tracks_running_anims() {
        let mut a = Animator::new();
        assert!(!a.is_active(), "初始无动画");
        let n = node(&[("transition", "width 200ms linear"), ("width", "0")]);
        a.set_now(0);
        a.apply(&n);
        let mut target = node(&[("transition", "width 200ms linear"), ("width", "100")]);
        a.set_now(0);
        a.apply(&target); // 启动 0→100
        assert!(a.is_active(), "动画进行中应为 active");
        a.set_now(1000); // 超过 duration
        let _ = a.apply(&target); // sample 到期移除动画
        assert!(
            !a.is_active(),
            "动画结束后应回到 inactive（允许无变化帧跳过重算）"
        );
    }

    #[test]
    fn animator_smooth_retarget() {
        let mut a = Animator::new();
        let mut n = node(&[("transition", "width 200ms linear"), ("width", "0")]);
        a.set_now(0);
        a.apply(&n);
        n.props.insert("width".into(), "100".into());
        a.set_now(0);
        a.apply(&n); // 开始 0→100
        a.set_now(100); // 中点 ≈50
        let mid = a.apply(&n);
        let v: f32 = mid.props["width"].parse().unwrap();
        assert!((v - 50.0).abs() < 2.0, "中点≈50, got {}", v);
        // 中途改目标 200，应从当前≈50 起而非从 0
        n.props.insert("width".into(), "200".into());
        a.set_now(100);
        let out = a.apply(&n);
        let v: f32 = out.props["width"].parse().unwrap();
        assert!((v - 50.0).abs() < 3.0, "retarget 起点应≈50, got {}", v);
    }

    #[test]
    fn parse_animation_blink() {
        let s = parse_animation("blink 1.06s infinite").expect("应解析 blink");
        assert_eq!(s.duration_ms, 1060);
        assert_eq!(s.iterations, -1);
        assert!(parse_animation("blink 500ms 3").is_some());
        assert!(parse_animation("pulse 1s").is_none(), "未内置动画名应忽略");
        assert!(parse_animation("blink").is_none(), "缺时长应忽略");
    }

    #[test]
    fn animation_blink_sample_square_wave() {
        let s = parse_animation("blink 1000ms infinite").unwrap();
        assert_eq!(s.sample_blink(0), Some(1.0));
        assert_eq!(s.sample_blink(499), Some(1.0));
        assert_eq!(s.sample_blink(500), Some(0.0));
        assert_eq!(s.sample_blink(999), Some(0.0));
        assert_eq!(s.sample_blink(1000), Some(1.0));
        let finite = parse_animation("blink 1000ms 2").unwrap();
        assert!(finite.sample_blink(1999).is_some());
        assert!(finite.sample_blink(2000).is_none(), "有限次数结束后不覆盖");
    }

    #[test]
    fn animator_applies_animation_opacity() {
        let mut a = Animator::new();
        a.set_now(0);
        let mut n = node(&[("animation", "blink 1000ms infinite")]);
        n.id = "caret".into();
        let out0 = a.apply(&n);
        assert_eq!(out0.props["opacity"], "1.000");
        assert!(a.has_active_animation());

        a.set_now(600);
        let out1 = a.apply(&n);
        assert_eq!(out1.props["opacity"], "0.000");
        // 常驻树不被污染
        assert!(!n.props.contains_key("opacity"));
    }

    #[test]
    fn animator_interpolates_color() {
        let mut a = Animator::new();
        let mut n = node(&[("transition", "bg 200ms linear"), ("bg", "#000000")]);
        n.id = "card".into();
        a.set_now(0);
        let out = a.apply(&n);
        assert_eq!(out.props["bg"], "#000000ff");
        n.props.insert("bg".into(), "#ffffff".into());
        a.set_now(1000);
        let out = a.apply(&n);
        assert_eq!(out.props["bg"], "#000000ff", "t=0 停在起点");
        a.set_now(1100);
        let out = a.apply(&n);
        assert_eq!(out.props["bg"], "#808080ff", "中点应为中灰");
        a.set_now(1200);
        let out = a.apply(&n);
        assert_eq!(out.props["bg"], "#ffffffff", "结束为白");
    }

    #[test]
    fn animator_interpolates_color_alpha() {
        let mut a = Animator::new();
        let mut n = node(&[("transition", "color 200ms linear"), ("color", "#ff000000")]);
        n.id = "fade".into();
        a.set_now(0);
        a.apply(&n);
        n.props.insert("color".into(), "#ff0000ff".into());
        a.set_now(1000);
        a.apply(&n);
        a.set_now(1100);
        let out = a.apply(&n);
        assert_eq!(out.props["color"], "#ff000080", "alpha 中点约 128");
    }

    #[test]
    fn animator_interpolates_transform() {
        let mut a = Animator::new();
        let mut n = node(&[
            ("transition", "transform 200ms linear"),
            ("transform", "translate(0px, 0px) scale(1)"),
        ]);
        n.id = "box".into();
        a.set_now(0);
        a.apply(&n);
        n.props.insert(
            "transform".into(),
            "translate(100px, -40px) scale(2)".into(),
        );
        a.set_now(1000); // 在 1000 启动动画
        a.apply(&n);
        a.set_now(1100); // 中点
        let out = a.apply(&n);
        let t = parse_transform(&out.props["transform"]);
        assert!((t.translate_x - 50.0).abs() < 1.0, "tx 中点≈50");
        assert!((t.translate_y - (-20.0)).abs() < 1.0, "ty 中点≈-20");
        assert!((t.scale - 1.5).abs() < 0.01, "scale 中点≈1.5");
    }

    #[test]
    fn animator_honors_delay() {
        let mut a = Animator::new();
        let mut n = node(&[("transition", "width 200ms linear 100ms"), ("width", "0")]);
        n.id = "delayed".into();
        a.set_now(0);
        a.apply(&n);
        n.props.insert("width".into(), "100".into());
        a.set_now(1000);
        let out = a.apply(&n);
        assert_eq!(out.props["width"], "0.000", "delay 期间停在起点");
        a.set_now(1099);
        let out = a.apply(&n);
        let v: f32 = out.props["width"].parse().unwrap();
        assert!(v < 1.0, "delay 未结束仍 near 起点");
        a.set_now(1200); // delay 100 + duration 200 → 动画中点 1000+100+100
        let out = a.apply(&n);
        let v: f32 = out.props["width"].parse().unwrap();
        assert!((v - 50.0).abs() < 2.0, "delay 后中点≈50，got {}", v);
        a.set_now(1300); // delay 100 + duration 200 结束
        let out = a.apply(&n);
        assert_eq!(out.props["width"], "100.000");
    }

    #[test]
    fn animator_all_expands_present_props() {
        let mut a = Animator::new();
        let mut n = node(&[
            ("transition", "all 200ms linear"),
            ("opacity", "0"),
            ("width", "0"),
            ("bg", "#000000"),
            ("transition-all-ignore", "keep"),
        ]);
        n.id = "allnode".into();
        a.set_now(0);
        a.apply(&n);
        n.props.insert("opacity".into(), "1".into());
        n.props.insert("width".into(), "50".into());
        n.props.insert("bg".into(), "#ffffff".into());
        n.props
            .insert("transition-all-ignore".into(), "changed".into());
        a.set_now(1000); // 启动动画
        a.apply(&n);
        a.set_now(1100); // 中点
        let out = a.apply(&n);
        let op: f32 = out.props["opacity"].parse().unwrap();
        let w: f32 = out.props["width"].parse().unwrap();
        assert!((op - 0.5).abs() < 0.02, "opacity 中点≈0.5");
        assert!((w - 25.0).abs() < 1.0, "width 中点≈25");
        assert_eq!(out.props["bg"], "#808080ff", "bg 中点灰");
        assert_eq!(
            out.props["transition-all-ignore"], "changed",
            "非白名单属性不被 all 动画改写"
        );
    }

    #[test]
    fn animator_color_retarget_smooth() {
        // 中途改向：从当前插值色起新动画，不跳变。
        let mut a = Animator::new();
        let mut n = node(&[("transition", "bg 200ms linear"), ("bg", "#000000")]);
        n.id = "rt".into();
        a.set_now(0);
        a.apply(&n);
        n.props.insert("bg".into(), "#ffffff".into());
        a.set_now(1000);
        a.apply(&n);
        a.set_now(1100); // 50% → 中灰
        let mid = a.apply(&n);
        let mid_color = mid.props["bg"].clone();
        // mid_color≈#808080，解析出当前通道作为改向起点。
        let (mr, _, _, _) = parse_color_rgba(&mid_color).unwrap();
        assert!((mr as i32 - 128).abs() <= 1, "中点应≈128，got {}", mr);
        // 改向到纯红：改向瞬间应从「当前色」平滑接续，不跳变。
        n.props.insert("bg".into(), "#ff0000".into());
        a.set_now(1101);
        let out = a.apply(&n);
        let (rr, _, _, _) = parse_color_rgba(&out.props["bg"]).unwrap();
        assert!((rr as i32 - mr as i32).abs() <= 1, "改向瞬间仍在当前色");
        a.set_now(1301); // 1101 + 200
        let out = a.apply(&n);
        assert_eq!(out.props["bg"], "#ff0000ff", "200ms 后到红色");
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    use crate::binprotocol::NodeType;

    #[test]
    fn parse_edges_shorthand() {
        assert_eq!(parse_edges("10"), Some([10.0, 10.0, 10.0, 10.0]));
        assert_eq!(parse_edges("10 20"), Some([10.0, 20.0, 10.0, 20.0]));
        assert_eq!(parse_edges("1 2 3"), Some([1.0, 2.0, 3.0, 2.0]));
        assert_eq!(parse_edges("1 2 3 4"), Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(parse_edges("10px 20px"), Some([10.0, 20.0, 10.0, 20.0]));
        assert_eq!(parse_edges("junk"), None);
        assert_eq!(parse_edges("1 2 x"), None);
    }

    #[test]
    fn whitespace_collapse_semantics() {
        assert_eq!(collapse_whitespace("  a \t b\nc  "), "a b c");
        assert!(white_space_collapses("normal"));
        assert!(white_space_collapses("NOWRAP"));
        assert!(!white_space_collapses(""));
        assert!(!white_space_collapses("pre"));
        assert!(!white_space_collapses("pre-wrap"));
    }

    #[test]
    fn font_weight_parsing() {
        let mut p = std::collections::HashMap::new();
        // font-weight 优先于 weight
        p.insert("font-weight".into(), "500".into());
        p.insert("weight".into(), "bold".into());
        assert_eq!(font_weight_from_props(&p), Some(500));
        p.insert("font-weight".into(), "bold".into());
        assert_eq!(font_weight_from_props(&p), Some(700));
        p.remove("font-weight");
        assert_eq!(font_weight_from_props(&p), Some(700), "weight 兜底");
        p.insert("weight".into(), "900".into());
        assert_eq!(font_weight_from_props(&p), Some(900));
        p.insert("weight".into(), "50".into());
        assert_eq!(font_weight_from_props(&p), Some(100), "clamp 到 100");
        p.insert("weight".into(), "normal".into());
        assert_eq!(font_weight_from_props(&p), Some(400));
    }

    #[test]
    fn text_align_line_height_helpers() {
        assert_eq!(text_align_of("center"), 1);
        assert_eq!(text_align_of("Right"), 2);
        assert_eq!(text_align_of("end"), 2);
        assert_eq!(text_align_of("start"), 0);
        assert_eq!(text_align_of("junk"), 0);
        assert_eq!(line_height_of("1.5"), (1.5, 0.0));
        assert_eq!(line_height_of("24px"), (0.0, 24.0));
        assert_eq!(line_height_of("30"), (0.0, 30.0), "无 px 的 30 视作绝对值");
        assert_eq!(line_height_of("junk"), (0.0, 0.0));
        assert!((align_offset(1, 100.0, 40.0) - 30.0).abs() < 0.01);
        assert!((align_offset(2, 100.0, 40.0) - 60.0).abs() < 0.01);
        assert_eq!(align_offset(0, 100.0, 40.0), 0.0);
        assert_eq!(align_offset(1, 40.0, 40.0), 0.0, "内容填满盒时无偏移");
    }

    #[test]
    fn z_index_parsed_into_style() {
        let mut props = std::collections::HashMap::new();
        props.insert("z-index".to_string(), "-5".to_string());
        let n = crate::scene::SceneNode {
            typ: NodeType::Div,
            kind: "div".into(),
            id: "n".into(),
            text: String::new(),
            nid: 1011,
            props,
            events: vec![],
            children: vec![],
        };
        assert_eq!(parse_style(&n).z_index, -5);
    }

    // ---- 描边 alpha 保留回归：Modal 发丝高光 #ffffff14 不得被画成纯白实线 ----

    #[test]
    fn parse_color_rgba_keeps_alpha_for_hairline_tokens() {
        // AppleTheme 真实 token：8% 白高光描边、60% 黑遮罩、95% 不透明面板。
        assert_eq!(parse_color_rgba("#ffffff14"), Some((255, 255, 255, 0x14)));
        assert_eq!(parse_color_rgba("#00000099"), Some((0, 0, 0, 0x99)));
        assert_eq!(parse_color_rgba("#1e1e1ef2"), Some((0x1e, 0x1e, 0x1e, 0xf2)));
        // 大小写不敏感；不带 alpha 的 6 位 hex 补 255。
        assert_eq!(parse_color_rgba("#FFFFFF14"), Some((255, 255, 255, 0x14)));
        assert_eq!(parse_color_rgba("#ffffff"), Some((255, 255, 255, 255)));
        // 3 位缩写扩位后 alpha=255。
        assert_eq!(parse_color_rgba("#fff"), Some((255, 255, 255, 255)));
        // 旧 parse_color 会丢掉 alpha——固化差异，防止有人用它解析 border。
        assert_eq!(crate::util::parse_color("#ffffff14"), (255, 255, 255));
    }

    #[test]
    fn parse_box_shadow_keeps_alpha() {
        // ShadowModal = "0 20px 56px 0 #00000066" 的 alpha 必须保留。
        let sh = parse_box_shadow("0 20px 56px 0 #00000066").unwrap();
        assert_eq!((sh.offset_y, sh.blur, sh.spread), (20.0, 56.0, 0.0));
        assert_eq!(sh.color, (0, 0, 0, 0x66));
    }
}
