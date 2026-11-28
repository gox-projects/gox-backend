package gxweb
import "testing"
func TestStickyPassthrough(t *testing.T) {
    if !isStyleKey("position") { t.Fatal("position 不在样式白名单") }
    if !isStyleKey("top") { t.Fatal("top 不在样式白名单") }
    if got := cssValue("position", "sticky"); got != "sticky" {
        t.Fatalf("position:sticky 被改写为 %q（应原样透传给浏览器）", got)
    }
    if got := cssValue("top", "0"); got != "0px" {
        t.Fatalf("top:0 应为 0px, got %q", got)
    }
}
