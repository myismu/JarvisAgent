// @vitest-environment happy-dom
/**
 * StreamingMarkdown.vue 增量流式渲染单元测试
 *
 * 覆盖：
 * 1. 跨段落增量追加：已冻结区块不再重复传入 marked.lexer，且 DOM 节点引用保持稳定
 * 2. 围栏代码块（含空行与跨 chunk 切分）：未闭合前绝不提前冻结，闭合后完整渲染
 * 3. 表格、引用块、列表渲染结构与原始 HTML 防 XSS 转义
 * 4. 内容整体替换与清空重置
 * 5. 2 万字长流式输出基准守卫：验证单次 lexer 输入长度始终受限于当前切片（O(Δ)）
 */
import { describe, it, expect, vi, afterEach } from "vitest";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { marked } from "marked";
import StreamingMarkdown from "../StreamingMarkdown.vue";

describe("StreamingMarkdown.vue", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("跨段落增量追加时仅解析新增冻结切片，已冻结 DOM 节点保持不变", async () => {
    const lexerSpy = vi.spyOn(marked, "lexer");
    const wrapper = mount(StreamingMarkdown, {
      props: { content: "第一段正文正在输入" },
    });

    expect(wrapper.findAll(".md-active-slot .md-paragraph")).toHaveLength(1);
    expect(wrapper.findAll(".md-block-slot:not(.md-active-slot)")).toHaveLength(0);

    // 跨过第一个空行：第一段冻结，第二段进入活跃区
    await wrapper.setProps({ content: "第一段正文正在输入\n\n第二段" });
    await nextTick();

    const frozenSlotsAfterFirst = wrapper.findAll(".md-block-slot:not(.md-active-slot)");
    expect(frozenSlotsAfterFirst).toHaveLength(1);
    const firstFrozenEl = frozenSlotsAfterFirst[0].element;
    expect(firstFrozenEl.textContent).toContain("第一段正文正在输入");

    // 继续向第二段追加字符：绝不应再把第一段送入 marked.lexer，且第一段 DOM 元素引用不变
    lexerSpy.mockClear();
    await wrapper.setProps({ content: "第一段正文正在输入\n\n第二段继续流式追加" });
    await nextTick();

    for (const [arg] of lexerSpy.mock.calls) {
      expect(String(arg)).not.toContain("第一段正文正在输入");
    }
    const frozenSlotsAfterSecond = wrapper.findAll(".md-block-slot:not(.md-active-slot)");
    expect(frozenSlotsAfterSecond).toHaveLength(1);
    expect(frozenSlotsAfterSecond[0].element).toBe(firstFrozenEl);
    expect(wrapper.find(".md-active-slot").text()).toContain("第二段继续流式追加");
  });

  it("未闭合围栏代码块含空行及跨 chunk 切分围栏时不提前冻结，闭合后正确渲染", async () => {
    const wrapper = mount(StreamingMarkdown, {
      props: { content: "前文段落\n\n``" },
    });
    await nextTick();
    // 前文段落已冻结，半截围栏还在活跃区
    expect(wrapper.findAll(".md-block-slot:not(.md-active-slot)")).toHaveLength(1);

    // 补全围栏开启并在代码块内部出现空行 \n\n：不能在代码块内部提前冻结
    const insideFence = "前文段落\n\n```ts\nconst a = 1;\n\nconst b = 2;\n";
    await wrapper.setProps({ content: insideFence });
    await nextTick();
    expect(wrapper.findAll(".md-block-slot:not(.md-active-slot)")).toHaveLength(1);

    // 闭合代码块并追加空行和下一段：此时整个代码块安全冻结
    const closed = `${insideFence}\`\`\`\n\n后续总结`;
    await wrapper.setProps({ content: closed });
    await nextTick();

    const frozenSlots = wrapper.findAll(".md-block-slot:not(.md-active-slot)");
    expect(frozenSlots).toHaveLength(2);
    expect(wrapper.find(".md-code-block .md-code-lang").text()).toBe("ts");
    expect(wrapper.find(".md-code-block code.language-ts").text()).toContain("const b = 2;");
    expect(wrapper.find(".md-active-slot").text()).toContain("后续总结");
  });

  it("正确渲染表格、引用块、列表并对原始 HTML 做安全转义", async () => {
    const md = [
      "## 标题二",
      "",
      "| 列A | 列B |",
      "| :--- | ---: |",
      "| 值1 | 值2 |",
      "",
      "> **提示**：引用内容",
      "",
      "- 列表项 1",
      "- 列表项 2",
      "",
      "<script>alert('xss')</script>",
    ].join("\n");

    const wrapper = mount(StreamingMarkdown, {
      props: { content: md },
    });
    await nextTick();

    expect(wrapper.find(".md-h2").text()).toBe("标题二");
    expect(wrapper.find(".md-table-wrap .md-table").exists()).toBe(true);
    expect(wrapper.findAll(".md-table tbody tr")).toHaveLength(1);
    expect(wrapper.find(".md-blockquote").text()).toContain("引用内容");
    expect(wrapper.findAll(".md-ul .md-li")).toHaveLength(2);
    // 绝不渲染可执行 <script> 标签
    expect(wrapper.find("script").exists()).toBe(false);
    expect(wrapper.html()).not.toContain("<script>");
  });

  it("内容整体替换或清空时彻底重置冻结区与活跃区", async () => {
    const wrapper = mount(StreamingMarkdown, {
      props: { content: "旧段落一\n\n旧段落二\n\n旧尾巴" },
    });
    await nextTick();
    expect(wrapper.findAll(".md-block-slot:not(.md-active-slot)")).toHaveLength(2);

    // 整体切换为不相关新内容
    await wrapper.setProps({ content: "全新内容\n\n新尾巴" });
    await nextTick();
    expect(wrapper.text()).not.toContain("旧段落一");
    expect(wrapper.findAll(".md-block-slot:not(.md-active-slot)")).toHaveLength(1);
    expect(wrapper.find(".md-active-slot").text()).toContain("新尾巴");

    // 清空内容
    await wrapper.setProps({ content: "" });
    await nextTick();
    expect(wrapper.findAll(".md-block-slot")).toHaveLength(0);
    expect(wrapper.text()).toBe("");
  });

  it("2 万字长流式输出基准：冻结区永不重复解析，单次 lexer 输入长度保持有界 O(1)", async () => {
    const lexerSpy = vi.spyOn(marked, "lexer");
    const wrapper = mount(StreamingMarkdown, {
      props: { content: "" },
    });

    // 构造 100 段 × ~220 字 ≈ 22,000 字符的长文本，分 200 次流式追加
    const paragraphBody = "这是一段用于测试流式 Markdown 增量渲染性能与复杂度的长文本内容。".repeat(7);
    let accumulated = "";
    let maxLexerInputLen = 0;

    for (let i = 1; i <= 100; i++) {
      const half1 = `第 ${i} 段：${paragraphBody.slice(0, 100)}`;
      const half2 = `${paragraphBody.slice(100)}\n\n`;

      accumulated += half1;
      await wrapper.setProps({ content: accumulated });

      accumulated += half2;
      await wrapper.setProps({ content: accumulated });
    }
    await nextTick();

    expect(accumulated.length).toBeGreaterThan(20000);
    for (const [input] of lexerSpy.mock.calls) {
      maxLexerInputLen = Math.max(maxLexerInputLen, String(input).length);
    }

    // 若退化为旧实现（每次跨段重解析 0..boundary），maxLexerInputLen 会高达 22000+；
    // 增量架构下每次只解析单个新增段或活跃段，应严格远小于全文长度
    expect(maxLexerInputLen).toBeLessThan(600);
    expect(wrapper.findAll(".md-block-slot:not(.md-active-slot)")).toHaveLength(100);
  });
});
