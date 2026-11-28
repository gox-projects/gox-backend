package rustbackend

import (
	"bytes"
	"encoding/binary"
	"strings"
	"testing"

	gxscene "github.com/robertpanvip/gox-core/runtime/scene"
)

// 本文件固化 GRX3 协议字节层的细粒度契约，与 grx3_test.go（镜像往返语义）
// 互补：这里逐字段验证编码布局、ID 解析边界、以及畸形字节喂给 Rust 侧
// 零拷贝解析时不 panic（事件泵线程存活是 GUI 交互的前提）。

// ---------- parseChangeID：字符串 ID → u64 直传的边界 ----------

func TestParseChangeIDBoundaries(t *testing.T) {
	cases := []struct {
		in    string
		want  uint64
		ok    bool
		label string
	}{
		{"", 0, true, "空串=NoNode=0"},
		{"0", 0, true, "零"},
		{"1", 1, true, "最小正数"},
		{"42", 42, true, "普通正数"},
		{"18446744073709551615", 18446744073709551615, true, "u64 最大值"},
		{"-1", 0, false, "负数非法"},
		{"abc", 0, false, "非数字非法"},
		{"12abc", 0, false, "数字+尾巴非法"},
		{" 7", 0, false, "前导空格非法（不 trim）"},
		{"18446744073709551616", 0, false, "超出 u64 溢出"},
		{"0x10", 0, false, "十六进制不识别"},
		{"1.5", 0, false, "小数非法"},
	}
	for _, c := range cases {
		got, ok := parseChangeID(c.in)
		if ok != c.ok || (ok && got != c.want) {
			t.Errorf("parseChangeID(%q) [%s] = (%d,%v), want (%d,%v)",
				c.in, c.label, got, ok, c.want, c.ok)
		}
	}
}

// ---------- serializeDeltaV3 字节布局 ----------

func TestGRX3DeltaByteLayout(t *testing.T) {
	root := &gxscene.Node{Type: gxscene.NodeDiv, ID: 1, Kind: "div"}
	changes := []gxscene.Change{
		{Kind: gxscene.ChangeSetText, NodeID: "2", Value: "hello"},
		{Kind: gxscene.ChangeSetText, NodeID: "3", Value: "world"},
	}
	b, ok := serializeDeltaV3(changes, root, 800, 600)
	if !ok {
		t.Fatal("serializeDeltaV3 应成功")
	}

	// Header 16B：magic / width / height / action_count / pool_len（宽高与 action 复用槽位见实现）。
	if len(b) < 16 {
		t.Fatalf("delta 太短: %d", len(b))
	}
	if got := binary.LittleEndian.Uint32(b[0:4]); got != deltaMagicV3 {
		t.Errorf("magic=0x%08X, want 0x%08X", got, deltaMagicV3)
	}
	if got := binary.LittleEndian.Uint32(b[4:8]); got != 800 {
		t.Errorf("width=%d, want 800", got)
	}
	if got := binary.LittleEndian.Uint32(b[8:12]); got != 2 {
		t.Errorf("action_count=%d, want 2", got)
	}

	// 每条动作 48B，node_id 在 o+8 的 u64 直传。
	for i, wantID := range []uint64{2, 3} {
		o := 16 + i*actionSizeV3
		if got := binary.LittleEndian.Uint64(b[o+8 : o+16]); got != wantID {
			t.Errorf("action[%d].node_id=%d, want %d", i, got, wantID)
		}
		if got := b[o]; got != 1 /* actSetText */ {
			t.Errorf("action[%d].kind=%d, want actSetText(1)", i, got)
		}
	}
	// deltas 不含全量帧 magic。
	if bytes.Contains(b[4:], []byte{0x47, 0x52, 0x58, 0x33}) {
		t.Error("delta 字节里不应出现 GRX3 全量帧标记串")
	}
}

func TestGRX3DeltaEmptyChanges(t *testing.T) {
	root := &gxscene.Node{Type: gxscene.NodeDiv, ID: 1, Kind: "div"}
	b, ok := serializeDeltaV3(nil, root, 800, 600)
	if !ok {
		t.Fatal("空 changes 应返回 ok=true（表示无需回退）")
	}
	if b != nil {
		t.Errorf("空 changes 应返回 nil 字节，实得 %d 字节", len(b))
	}
}

func TestGRX3DeltaAllActionKinds(t *testing.T) {
	root := &gxscene.Node{Type: gxscene.NodeDiv, ID: 1, Kind: "div", Children: []*gxscene.Node{
		{Type: gxscene.NodeText, ID: 2, Text: "a"},
		{Type: gxscene.NodeText, ID: 3, Text: "b"},
	}}
	added := &gxscene.Node{Type: gxscene.NodeText, ID: 4, Text: "c"}
	changes := []gxscene.Change{
		{Kind: gxscene.ChangeSetAttr, NodeID: "2", Key: "color", Value: "red"},
		{Kind: gxscene.ChangeSetText, NodeID: "2", Value: "A2"},
		{Kind: gxscene.ChangeSetHandler, NodeID: "2", Key: "click"},
		{Kind: gxscene.ChangeRemoveChild, NodeID: "1", ChildID: "3"},
		{Kind: gxscene.ChangeMoveChild, NodeID: "1", ChildID: "2", BeforeID: ""},
		{Kind: gxscene.ChangeInsertBefore, NodeID: "1", ChildID: "4", BeforeID: "", Value: added},
	}
	b, ok := serializeDeltaV3(changes, root, 800, 600)
	if !ok {
		t.Fatal("所有合法 action 类型都应编码成功")
	}
	if len(b) < 16+len(changes)*actionSizeV3 {
		t.Fatalf("delta 长度不足以容纳全部动作: %d", len(b))
	}
	if got := binary.LittleEndian.Uint32(b[8:12]); got != uint32(len(changes)) {
		t.Errorf("action_count=%d, want %d", got, len(changes))
	}

	// 喂给 Rust 镜像探针：合法 delta 应能被消费且不 panic。
	mirrorReset()
	frame := mustFrameV3(t, root)
	if rc := mirrorApply(frame); rc != 0 {
		t.Fatalf("建树失败 rc=%d", rc)
	}
	mirrorApply(b) // 增量应用；细节断言见 grx3_test.go，这里只保证不 panic
}

func TestGRX3DeltaNonNumericIDsRejected(t *testing.T) {
	root := &gxscene.Node{Type: gxscene.NodeDiv, ID: 1, Kind: "div"}
	badChanges := [][]gxscene.Change{
		{{Kind: gxscene.ChangeSetText, NodeID: "not-num", Value: "x"}},
		{{Kind: gxscene.ChangeRemoveChild, NodeID: "1", ChildID: "bad-child"}},
		{{Kind: gxscene.ChangeInsertBefore, NodeID: "1", ChildID: "2", BeforeID: "oops",
			Value: &gxscene.Node{Type: gxscene.NodeText, ID: 2, Text: "x"}}},
	}
	for i, changes := range badChanges {
		if _, ok := serializeDeltaV3(changes, root, 800, 600); ok {
			t.Errorf("case %d: 非数字 ID 应导致编码失败（回退全量）", i)
		}
	}
}

// ---------- 畸形字节喂 Rust 零拷贝解析：绝不 panic ----------

func TestMalformedGRX3BytesDoNotCrashRust(t *testing.T) {
	mirrorReset()
	cases := [][]byte{
		nil,
		{},
		{0x48, 0x52, 0x58, 0x33},                 // 只有 magic
		bytes.Repeat([]byte{0xFF}, 16),            // 非法 magic + 垃圾
		{0x48, 0x52, 0x58, 0x33, 0xFF, 0xFF, 0xFF, 0xFF}, // magic 对但 count 巨大
	}
	for i, buf := range cases {
		// mirrorApply 内部是 Rust 零拷贝解析；坏帧返回非 0，但绝不能 panic 杀死进程。
		rc := mirrorApply(buf)
		t.Logf("malformed case %d: rc=%d (非 0 属正常拒绝)", i, rc)
	}
	// 经过所有坏帧后进程仍存活、仍能正常建树 → 证明事件泵线程未被杀死。
	root := buildTree(5)
	if rc := mirrorApply(mustFrameV3(t, root)); rc != 0 {
		t.Fatalf("坏帧之后镜像仍应可用，建树 rc=%d", rc)
	}
	if got := mirrorNodeCount(); got != 5 {
		t.Fatalf("坏帧之后镜像节点数=%d, want 5", got)
	}
}

func TestGRX3MagicBytesStability(t *testing.T) {
	// 全量帧 magic "GRX3" 与增量帧 magic "GRX3D" 必须在字节层可区分。
	full := []byte{0x47, 0x52, 0x58, 0x33} // "GRX3" = 0x33585247 LE
	if got := binary.LittleEndian.Uint32(full); got != binMagicV3 {
		t.Errorf("全量帧 magic 字节与 binMagicV3(0x%08X) 不一致，实得 0x%08X", binMagicV3, got)
	}
	if deltaMagicV3 == binMagicV3 {
		t.Fatal("GRX3 增量帧 magic 不得等于全量帧 magic")
	}
	// 增量 magic 末字节应比全量帧多 D（0x48 vs 0x47）。
	fullB := make([]byte, 4)
	binary.LittleEndian.PutUint32(fullB, binMagicV3)
	deltaB := make([]byte, 4)
	binary.LittleEndian.PutUint32(deltaB, deltaMagicV3)
	if fullB[0] == deltaB[0] {
		t.Error("全量/增量 magic 最低字节应不同（0x47 vs 0x48）")
	}
}

func TestParseChangeIDRoundtripWithNodeIDString(t *testing.T) {
	// 场景 NodeID.String() 产出的十进制串应能被 parseChangeID 无损还原。
	for _, id := range []gxscene.NodeID{0, 1, 7, 100, 1 << 40, ^gxscene.NodeID(0)} {
		s := id.String()
		got, ok := parseChangeID(s)
		if !ok || got != uint64(id) {
			t.Errorf("NodeID %d 的字符串 %q 往返失败: got (%d,%v)", id, s, got, ok)
		}
	}
}

// ---------- delta 往返文本不丢失（细粒度逐值）----------

func TestGRX3DeltaTextValuesSurviveRoundtrip(t *testing.T) {
	mirrorReset()
	root := &gxscene.Node{Type: gxscene.NodeDiv, ID: 1, Kind: "div", Children: []*gxscene.Node{
		{Type: gxscene.NodeText, ID: 2, Text: "old"},
	}}
	if rc := mirrorApply(mustFrameV3(t, root)); rc != 0 {
		t.Fatalf("建树失败 rc=%d", rc)
	}
	// 多字节 UTF-8 + 空串 + 特殊字符，验证 pool_str 的 UTF-8 处理。
	for _, text := range []string{"hello", "中文测试", "", "a\tb\nc", "emoji🚀"} {
		changes := []gxscene.Change{{Kind: gxscene.ChangeSetText, NodeID: "2", Value: text}}
		b, ok := serializeDeltaV3(changes, root, 800, 600)
		if !ok {
			t.Fatalf("文本 %q 编码失败", text)
		}
		if rc := mirrorApply(b); rc != 0 {
			t.Fatalf("文本 %q delta 应用失败 rc=%d", text, rc)
		}
		if text != "" {
			dump := mirrorDump()
			if !strings.Contains(dump, text) {
				t.Errorf("镜像摘要缺文本 %q: %.200s", text, dump)
			}
		}
	}
}
