package gxweb

import (
	"encoding/json"
	"fmt"
	"hash/fnv"
	"strconv"
	"strings"

	"github.com/robertpanvip/gox-core/cssbox"
	gxscene "github.com/robertpanvip/gox-core/runtime/scene"
)

// isStyleKey 判断 key 是否为受限 CSS sheet 属性（与 gxscene.StylePropNames 一致，
// 额外兼容少量常用键；伪状态子表 "&:x" 也归此列）。纯字符串判断，两驱动共享。
func isStyleKey(key string) bool {
	switch strings.ToLower(key) {
	case "color", "bg", "background", "width", "height", "font-size", "font-family", "weight",
		"padding", "border-radius", "border", "border-width", "opacity",
		"flex-direction", "flex-wrap", "justify-content", "align-items", "align-content",
		"gap", "row-gap", "column-gap", "position", "top", "right", "bottom", "left",
		"overflow", "scroll-top", "scroll-left", "transform", "transition", "box-shadow",
		"flex-grow", "flex-shrink", "min-width", "max-width", "min-height", "max-height",
		"margin", "margin-top", "margin-bottom", "margin-left", "margin-right",
		"text-align", "line-height", "letter-spacing", "z-index", "cursor",
		"white-space", "animation", "animation-keyframes", "pointer-events":
		return true
	}
	if isPseudoElementKey(key) {
		return false // 伪元素走独立规则路径，不是内联样式键
	}
	return strings.HasPrefix(key, "&:") || strings.HasPrefix(key, "$:")
}

// isGradientTextProp 判断属性是否为「渐变文字」语义开关（三端统一）。
// Skia/Rust 后端据此把 bg 渐变裁到文字而非画成背景；Web 端在 SetAttr 里
// 据此生成 background-clip:text + color:transparent，避免编辑器依赖宿主 CSS。
// isPseudoElementKey 判断 key 是否为伪元素子表协议键（"&::x"）。
// 双冒号与伪状态 "&:x" 严格区分，走独立的原生 CSS 规则路径。
func isPseudoElementKey(key string) bool {
	return strings.HasPrefix(key, "::") || strings.HasPrefix(key, "&::")
}

// isSelectionKey 判断是否为 ::selection 伪元素子表。
func isSelectionKey(key string) bool {
	return key == "&::selection" || key == "::selection"
}

// selectionVars 把 "&::selection" 子表 JSON 串解析成要写到元素上的 CSS 自定义
// 属性值 (background, foreground)。空值表示该属性未声明。
//
// ::selection 无法通过内联 style 直接设置，只能靠一条全局规则读取元素上的
// 自定义属性（见 selectionGlobalCSS）。未声明的元素不写这两个变量，全局规则的
// var() 无值时该声明被浏览器忽略，因此不影响默认选区外观。
func selectionVars(subJSON string) (bg, fg string) {
	if subJSON == "" || subJSON == "{}" {
		return "", ""
	}
	var sub map[string]string
	if err := json.Unmarshal([]byte(subJSON), &sub); err != nil {
		return "", ""
	}
	bg = sub["bg"]
	if bg == "" {
		bg = sub["background"]
	}
	fg = sub["color"]
	return bg, fg
}

// selectionGlobalCSS 是 Web 端注入一次的全局 ::selection 规则。各元素通过内联
// 自定义属性 --gx-sel-bg / --gx-sel-fg 声明自己的选区色；变量缺省时 var() 无值，
// 浏览器保留原生选区，故不影响未使用 selection 子表的元素。
const selectionGlobalCSS = "::selection { background-color: var(--gx-sel-bg); color: var(--gx-sel-fg); }"

// animationGlobalCSS 是 Web 端注入一次的全局 @keyframes。三端统一的内置动画
// （当前仅 blink）在此定义关键帧；元素的 animation prop 只引用 gx- 前缀的帧名，
// 由浏览器合成器原生驱动，无需宿主 CSS 或 JS/RAF。
//
// blink 与 gxscene.AnimationSpec.Sample 严格对齐：相位 < 50% opacity=1，
// 否则 opacity=0（steps(1) 让 49%/50% 处阶跃，不做线性插值）。
const animationGlobalCSS = "@keyframes gx-blink { 0%,49% { opacity: 1; } 50%,100% { opacity: 0; } }"

// animationCSS 把声明式 animation prop（如 "blink 1.06s infinite"）翻译成
// 原生 CSS animation 值（如 "gx-blink 1.06s steps(1) infinite"）。
// 非法/空值返回空串，让调用方移除 animation 属性。
func animationCSS(value string) string {
	return animationCSSWithKeyframes(value, "")
}

// animationCSSWithKeyframes 在 animationCSS 基础上支持自定义关键帧帧名：
// kfName 非空时用 "gx-<hash>" 作为 animation-name（自定义关键帧），timing 用 linear；
// 否则走内置 blink 路径。自定义动画的简写串可能没有 name（如 "1000ms -1"），
// 此时 ParseAnimation 不 Active，但只要有 kfName 就应产出 CSS。
func animationCSSWithKeyframes(value, kfName string) string {
	spec := gxscene.ParseAnimation(value)
	durMS := spec.DurationMS
	iterations := spec.Iterations
	if kfName != "" {
		// 自定义动画：从简写串兜底解析时长/迭代（name 位可能是时长）。
		if durMS <= 0 {
			durMS = animationDurationFallback(value)
		}
		if iterations == 0 {
			iterations = animationIterationsFallback(value)
		}
		if durMS <= 0 {
			return ""
		}
		dur := float64(durMS) / 1000
		durStr := strconv.FormatFloat(dur, 'g', -1, 64) + "s"
		iter := strconv.Itoa(iterations)
		if iterations < 0 {
			iter = "infinite"
		}
		return kfName + " " + durStr + " linear " + iter
	}
	if !spec.Active() {
		return ""
	}
	dur := float64(spec.DurationMS) / 1000
	durStr := strconv.FormatFloat(dur, 'g', -1, 64) + "s"
	iter := strconv.Itoa(spec.Iterations)
	if spec.Iterations < 0 {
		iter = "infinite"
	}
	// blink 用 steps(1) 实现方波；预留给未来其它内置动画的 timing 分支。
	timing := "linear"
	if spec.Name == gxscene.AnimationBlink {
		timing = "steps(1)"
	}
	return "gx-" + spec.Name + " " + durStr + " " + timing + " " + iter
}

// animationDurationFallback 从简写串里提取时长（毫秒）：
// 自定义动画串形如 "1000ms -1" / "1s infinite"，name 位缺失。
func animationDurationFallback(value string) int64 {
	for _, tok := range strings.Fields(strings.ToLower(strings.TrimSpace(value))) {
		if tok == "infinite" {
			continue
		}
		// 迭代次数（纯数字）跳过。
		if _, err := strconv.Atoi(tok); err == nil {
			continue
		}
		if v, ok := parseWebDurationMS(tok); ok {
			return v
		}
	}
	return 0
}

// animationIterationsFallback 从简写串兜底解析迭代次数：-1/infinite → -1；
// 正整数 → 该数；缺省 -1。时长 token（带 s/ms）跳过。
func animationIterationsFallback(value string) int {
	for _, tok := range strings.Fields(strings.ToLower(strings.TrimSpace(value))) {
		if tok == "infinite" || tok == "-1" {
			return -1
		}
		if strings.HasSuffix(tok, "ms") || strings.HasSuffix(tok, "s") {
			continue
		}
		if n, err := strconv.Atoi(tok); err == nil && n > 0 {
			return n
		}
	}
	return -1
}

func parseWebDurationMS(s string) (int64, bool) {
	s = strings.TrimSpace(s)
	if strings.HasSuffix(s, "ms") {
		v, err := strconv.ParseFloat(strings.TrimSuffix(s, "ms"), 64)
		return int64(v), err == nil
	}
	if strings.HasSuffix(s, "s") {
		v, err := strconv.ParseFloat(strings.TrimSuffix(s, "s"), 64)
		return int64(v * 1000), err == nil
	}
	v, err := strconv.ParseFloat(s, 64)
	return int64(v * 1000), err == nil
}

// keyframesCSSName 基于关键帧 JSON 生成稳定的帧名（内容 hash，避免同名冲突）。
func keyframesCSSName(jsonText string) string {
	h := fnv.New64a()
	_, _ = h.Write([]byte(jsonText))
	return fmt.Sprintf("gx-kf-%x", h.Sum64())
}

// buildKeyframesCSS 把 animation-keyframes JSON 翻译成原生 @keyframes 文本：
//
//	[{"offset":"0","props":{"opacity":"0","bg":"#000"}},...]
//	  → @keyframes gx-kf-xxx { 0% { opacity:0; background-color:#000; } ... }
//
// keyframesJSON 为空/非法返回空串。属性 kebab key 映射成 CSS 属性名（bg→background-color）。
func buildKeyframesCSS(jsonText, name string) string {
	if strings.TrimSpace(jsonText) == "" {
		return ""
	}
	var frames []struct {
		Offset json.RawMessage            `json:"offset"`
		Props  map[string]json.RawMessage `json:"props"`
	}
	if err := json.Unmarshal([]byte(jsonText), &frames); err != nil || len(frames) == 0 {
		return ""
	}
	var rules []string
	for _, f := range frames {
		pct := keyframeOffsetPercent(f.Offset)
		if pct < 0 {
			continue
		}
		var decls []string
		for k, raw := range f.Props {
			cssKey := keyframePropName(k)
			if cssKey == "" {
				continue
			}
			var v any
			if err := json.Unmarshal(raw, &v); err != nil {
				continue
			}
			var val string
			switch x := v.(type) {
			case string:
				val = x
			case float64:
				val = strconv.FormatFloat(x, 'g', -1, 64)
			default:
				continue
			}
			decls = append(decls, cssKey+":"+val+";")
		}
		if len(decls) == 0 {
			continue // 无可用属性的帧不生成空规则
		}
		rules = append(rules, pctStr(pct)+"{"+strings.Join(decls, "")+"}")
	}
	if len(rules) == 0 {
		return ""
	}
	return "@keyframes " + name + " {" + strings.Join(rules, "") + "}"
}

// keyframeOffsetPercent 把 offset（数字 0..1 或 "0%"/"50%"）转成百分数 0..100。
func keyframeOffsetPercent(raw json.RawMessage) float64 {
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		s = strings.TrimSpace(s)
		if strings.HasSuffix(s, "%") {
			v, err := strconv.ParseFloat(strings.TrimSuffix(s, "%"), 64)
			if err != nil {
				return -1
			}
			return v
		}
		v, err := strconv.ParseFloat(s, 64)
		if err != nil {
			return -1
		}
		return v * 100
	}
	var f float64
	if err := json.Unmarshal(raw, &f); err == nil {
		return f * 100
	}
	return -1
}

// keyframePropName 把后端 kebab 属性名映射成 CSS 属性名；不可动画/未知返回空串。
func keyframePropName(k string) string {
	switch strings.ToLower(k) {
	case "opacity":
		return "opacity"
	case "transform":
		return "transform"
	case "color":
		return "color"
	case "bg", "background":
		return "background-color"
	case "width":
		return "width"
	case "height":
		return "height"
	case "font-size":
		return "font-size"
	case "border-radius":
		return "border-radius"
	default:
		return ""
	}
}

func pctStr(p float64) string {
	return strconv.FormatFloat(p, 'g', -1, 64) + "%"
}

// 选区自定义属性名（内联写到元素 style 上）。
const (
	selectionVarBg = "--gx-sel-bg"
	selectionVarFg = "--gx-sel-fg"
)

func isGradientTextProp(key string) bool {
	return strings.EqualFold(key, "gradient-text")
}

// gradientTextEnabled 判断渐变文字开关的值是否表示开启（"1"/"true"）。
func gradientTextEnabled(value string) bool {
	return value == "1" || strings.EqualFold(value, "true")
}

// cssName 把 GX 样式名映射到 CSS 属性名。数值型长度由 cssValue 加 px。
func cssName(key string) string {
	switch key {
	case "bg":
		return "background"
	case "weight":
		return "font-weight"
	case "scroll-top":
		return "scrollTop"
	case "scroll-left":
		return "scrollLeft"
	}
	return key
}

// styleTargetProp 返回设置样式时实际使用的 CSS 属性名。
//
// 关键：bg=linear-gradient(...) 必须用 background-image 而非 background 简写。
// CSS 的 background 简写会把 background-clip 重置为 border-box，若与
// gradient-text 的 background-clip:text 同时出现，无论两者设置顺序如何，
// 渐变都会被画成背景色块、文字被 color:transparent 藏掉（截图表现为一条条
// 彩色方块）。用 background-image 只设背景图，不触碰 clip，编辑器渐变文字
// 因此完全由 GX 语义属性表达，无需宿主 CSS 兜底。
func styleTargetProp(key, value string) string {
	name := cssName(key)
	if name == "background" && strings.HasPrefix(strings.TrimSpace(value), "linear-gradient(") {
		return "background-image"
	}
	return name
}

// boxShorthandProps 是支持 1~4 值简写的盒模型属性。这些属性的值先经 cssbox
// 解析成四边，再逐边补 px 拼回 CSS 简写串；避免 "7 14" 被整串 ParseFloat 失败后
// 输出无单位 CSS（浏览器忽略）导致 padding / margin 失效。
var boxShorthandProps = map[string]bool{
	"padding": true, "margin": true, "border-radius": true, "gap": true,
}

// dimensionlessProps 是无量纲属性，数值不加 px。
var dimensionlessProps = map[string]bool{
	"opacity": true, "weight": true, "z-index": true,
	"flex-grow": true, "flex-shrink": true, "line-height": true, "order": true,
}

// isFlexLayoutKey 判定一个 GX 样式属性是否表达了 flex 布局意图。
// 浏览器只在 display:flex 容器上解释这些属性，故 js 驱动设置它们时需同步补
// display:flex（见 dom_js.go SetAttr），否则 GX 场景树在 Web 上退化为块级堆叠。
func isFlexLayoutKey(key string) bool {
	switch strings.ToLower(key) {
	case "flex-direction", "flex-wrap", "justify-content", "align-items",
		"align-content", "gap", "row-gap", "column-gap", "flex-grow", "flex-shrink":
		return true
	}
	return false
}

// flexDefaultColumn 报告设置 flex 意图属性 key 时，是否需要给容器补 GX 三端
// 约定的默认主轴 flex-direction:column。Skia（flexDirection 零值）与 Rust
// （Taffy unwrap_or("column")）的 flex 容器默认都是列向；浏览器初始值是 row，
// 不补声明的话，带 flex 意图属性但未显式声明方向的容器在 Web 上会横排，
// 与两端布局不一致。existing 是元素当前已生效的 flex-direction 值（空=未声明），
// 已显式声明（含 row）时不覆盖。native 单测覆盖本函数，js 驱动调用它。
func flexDefaultColumn(key, existing string) bool {
	if strings.EqualFold(key, "flex-direction") {
		return false
	}
	return strings.TrimSpace(existing) == ""
}

// expandBoxShadow 把单层 GX box-shadow token 展开成复刻 Skia/Rust 24 层矩形
// 近似的 CSS 多层阴影，使 Web 端与桌面/Rust 端的阴影视觉一致。
//
// Skia paintShadow（Rust build_gpu_frame 同算法）：
//
//	baseA = 颜色 alpha（<=0 时取 90）
//	for i = 24..1:
//	    t      = i / 24
//	    expand = spread + blur * t
//	    a      = baseA * (1 - t) / 24 * 1.8   （clamp 0..255）
//	    画圆角矩形：四周扩 expand，alpha=a，radius=原 radius+expand
//
// 层数必须足够多：6 层时层间距 blur/6（blur=56 时约 9px），每层硬边叠加会
// 形成肉眼可见的同心"靶环"台阶；24 层把间距降到 ~2.3px，剖面平滑递减且贴近
// 高斯参照。alpha 系数 1.8 保持不变即可维持与旧 6 层基本一致的总阴影深度。
//
// CSS box-shadow 的 spread 会同步把圆角半径增大 spread，故每层用
// "ox oy 0px expand rgba(r,g,b,a/255)" 精确对应 Skia 的一层实心圆角矩形。
// 输出顺序 i=1..24：CSS 第一个阴影绘制在最上层，对应 Skia 最后画的、最浓的内层。
//
// 已是多层（含逗号）或解析失败时原样透传，避免二次展开。
func expandBoxShadow(v string) string {
	ox, oy, blur, spread, r, g, b, baseA, ok := parseShadowToken(v)
	if !ok {
		return v
	}
	if baseA <= 0 {
		baseA = 90
	}
	const layers = 24
	parts := make([]string, 0, layers)
	for i := 1; i <= layers; i++ {
		t := float64(i) / layers
		expand := spread + blur*t
		a := float64(baseA) * (1 - t) / layers * 1.8
		if a < 0 {
			a = 0
		} else if a > 255 {
			a = 255
		}
		parts = append(parts, fmt.Sprintf("%gpx %gpx 0px %gpx rgba(%d,%d,%d,%.4f)",
			ox, oy, expand, r, g, b, a/255))
	}
	return strings.Join(parts, ", ")
}

// parseShadowToken 解析 "ox oy [blur] [spread] [color]"。color 缺省为半透明黑，
// 与 Skia parseBoxShadow 的默认（alpha=90）一致。
func parseShadowToken(v string) (ox, oy, blur, spread float64, r, g, b, a int, ok bool) {
	v = strings.TrimSpace(v)
	if v == "" || strings.Contains(v, ",") {
		return // 已多层或空：不展开
	}
	fields := strings.Fields(v)
	if len(fields) < 2 {
		return
	}
	nums := make([]float64, 0, 4)
	colorStr := ""
	for _, f := range fields {
		n, err := strconv.ParseFloat(strings.TrimSuffix(strings.TrimSuffix(f, "px"), "PX"), 64)
		if err == nil && len(nums) < 4 {
			nums = append(nums, n)
			continue
		}
		colorStr = f
	}
	if len(nums) < 2 {
		return
	}
	ox, oy = nums[0], nums[1]
	if len(nums) >= 3 {
		blur = nums[2]
	}
	if len(nums) >= 4 {
		spread = nums[3]
	}
	a = 90 // Skia 默认半透明黑
	if colorStr != "" {
		r, g, b, a = parseShadowColor(colorStr)
	}
	ok = true
	return
}

// parseShadowColor 解析阴影颜色：#rgb / #rrggbb / #rrggbbaa / rgb() / rgba()。
// 无法识别时返回黑色 + alpha=90（与 Skia 兜底一致）。
func parseShadowColor(s string) (r, g, b, a int) {
	s = strings.ToLower(strings.TrimSpace(s))
	a = 90
	if strings.HasPrefix(s, "#") {
		h := s[1:]
		switch len(h) {
		case 3:
			r, g, b = hexByte(h[0:1])*17, hexByte(h[1:2])*17, hexByte(h[2:3])*17
			a = 255
		case 6:
			r, g, b = hexByte(h[0:2]), hexByte(h[2:4]), hexByte(h[4:6])
			a = 255
		case 8:
			r, g, b, a = hexByte(h[0:2]), hexByte(h[2:4]), hexByte(h[4:6]), hexByte(h[6:8])
		}
		return
	}
	if strings.HasPrefix(s, "rgba(") || strings.HasPrefix(s, "rgb(") {
		inner := s[strings.Index(s, "(")+1 : strings.LastIndex(s, ")")]
		parts := strings.Split(inner, ",")
		if len(parts) >= 3 {
			r, _ = strconv.Atoi(strings.TrimSpace(parts[0]))
			g, _ = strconv.Atoi(strings.TrimSpace(parts[1]))
			b, _ = strconv.Atoi(strings.TrimSpace(parts[2]))
			a = 255
			if len(parts) >= 4 {
				af, err := strconv.ParseFloat(strings.TrimSpace(parts[3]), 64)
				if err == nil {
					a = int(af * 255)
				}
			}
		}
	}
	return
}

func hexByte(s string) int {
	v, _ := strconv.ParseUint(s, 16, 8)
	return int(v)
}

// cssValue 把样式值规整为 CSS 值：纯数字的长度属性补 px；盒模型简写属性支持
// 1~4 值逐边补单位；box-shadow 展开为 24 层以对齐 Skia/Rust；颜色/关键字原样。
func cssValue(key, v string) string {
	if v == "" {
		return v
	}
	if key == "box-shadow" {
		return expandBoxShadow(v)
	}
	if boxShorthandProps[key] {
		if edges, ok := cssbox.Values(v); ok {
			return cssbox.Shorthand(cssbox.WithUnit(edges))
		}
		return v // 非数值简写（如 auto）原样透传
	}
	if _, err := strconv.ParseFloat(v, 64); err != nil {
		return v
	}
	if dimensionlessProps[key] {
		return v // 无量纲
	}
	return v + "px"
}
