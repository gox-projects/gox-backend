package skia

import (
	gxscene "github.com/robertpanvip/gox-core/runtime/scene"
	"testing"
)

// stickyTree：overflow 容器高 60，内含 4 个高 40 的子块；第 2 个 sticky top=10。
// 未滚动时自然位置 y=40；scrollTop=30 自然 y=10；scrollTop=60 自然 y=-20，
// 此时应被吸附到视口顶部 + top = 0 + 10 = 10。
func stickyTree(scrollTop float32) *gxscene.Node {
	vp := elemNode("div", map[string]any{
		"width": 200, "height": 60, "overflow": "scroll", "scroll-top": scrollTop,
	})
	for i := 0; i < 4; i++ {
		props := map[string]any{"width": 200, "height": 40, "bg": "#ffffff"}
		if i == 1 {
			props["position"] = "sticky"
			props["top"] = 10
		}
		vp.Children = append(vp.Children, elemNode("div", props, textNode(itemLabel(i), nil)))
	}
	return vp
}

func TestParsePositionSticky(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want positionMode
	}{
		{"sticky", positionSticky},
		{" Sticky ", positionSticky},
		{"absolute", positionAbsolute},
		{"relative", positionRelative},
		{"nonsense", positionRelative},
	} {
		got := parseStyle(map[string]any{"position": tc.in}).position
		if got != tc.want {
			t.Errorf("position=%q got %v want %v", tc.in, got, tc.want)
		}
	}
}

func TestStickyTopNoScrollKeepsNatural(t *testing.T) {
	// scrollTop=0：sticky 自然 y=40，视口顶吸附线=0+10=10，不应被上移。
	vp := stickyTree(0)
	cache := computeLayout(vp, 800, 600, fixedMeasure2)
	sticky := cache[vp.Children[1]]
	if !near(sticky.Y, 40, 2) {
		t.Errorf("未滚动时 sticky 应保持自然位置 y≈40, got %.1f", sticky.Y)
	}
}

func TestStickyTopSticksOnScroll(t *testing.T) {
	// scrollTop=60：第 2 块自然 y=40-60=-20，应被吸附到视口顶 + top=10。
	vp := stickyTree(60)
	cache := computeLayout(vp, 800, 600, fixedMeasure2)
	sticky := cache[vp.Children[1]]
	if !near(sticky.Y, 10, 2) {
		t.Errorf("滚动后 sticky 应吸附到 y≈10, got %.1f", sticky.Y)
	}
	// 普通兄弟节点不受影响：第 1 块自然 y=0-60=-60。
	first := cache[vp.Children[0]]
	if !near(first.Y, -60, 2) {
		t.Errorf("普通兄弟节点不应被 sticky 影响, y=%.1f want ≈-60", first.Y)
	}
	// sticky 自身的子树应跟着吸附一起平移：其文本子节点相对偏移不变。
	// 文本 y = sticky.Y + LayoutTop(文本在块内)。
	if txt := vp.Children[1].Children; len(txt) > 0 {
		ty := cache[txt[0]].Y
		if ty < sticky.Y-1 {
			t.Errorf("sticky 子树应跟随吸附, text y=%.1f sticky y=%.1f", ty, sticky.Y)
		}
	}
}

func TestStickyBottomSticks(t *testing.T) {
	// bottom=5：自然位置超出视口底部时吸附到 视口底 - bottom - 自身高。
	vp := elemNode("div", map[string]any{
		"width": 200, "height": 60, "overflow": "scroll", "scroll-top": 0,
	})
	stickyNode := elemNode("div", map[string]any{
		"width": 200, "height": 20, "position": "sticky", "bottom": 5,
	}, textNode("S", nil))
	// 前置内容把 sticky 推到 y=100（远超视口底 60）。
	vp.Children = append(vp.Children,
		elemNode("div", map[string]any{"width": 200, "height": 100}),
		stickyNode,
	)
	cache := computeLayout(vp, 800, 600, fixedMeasure2)
	got := cache[stickyNode].Y
	want := float32(60 - 5 - 20) // 视口底(60) - bottom(5) - 高(20) = 35
	if !near(got, want, 2) {
		t.Errorf("bottom 吸附 y=%.1f want ≈%.1f", got, want)
	}
}

// 确保 gxscene 导入被使用（elemNode/textNode 已在其它测试用到，这里显式引用类型）
var _ *gxscene.Node
