<script setup lang="ts">
// 会话导航条（Codex 风格 minimap / outline rail）
//
// 原理：不是滚动条，是「会话章节目录」的可视化——
//   1. 建索引：**只采集 user 消息**（.chat-message.user-message）。
//      用户回历史是按"自己问过什么"定位的，user 消息就是对话的天然章节锚点；
//      agent 回复紧跟在提问之后，跳到章节头即可看到整轮回复，单独成条目只是噪音。
//      附带收益：user 消息发出后内容永不变更，流式输出期间导航条几乎零重算。
//      摘录取 .user-text 原文（排除"展开全部"按钮等杂音），预览卡 = 会话目录。
//      选 DOM 而非消息数据作为数据源：天然与渲染结果一一对应（点击跳转、
//      高亮定位都靠元素）。
//   2. 画横线：**固定小间距**（~10px 一格）紧凑排布并垂直居中（Codex 同款），
//      条目极多时间距压缩到最小 4px，永不溢出；悬停时横线弹性变长。
//   3. 联动：滚动容器 scroll 事件 + MutationObserver（rAF 节流）→
//      以视口上部 35% 处为探测线，最后越过探测线的条目（= 当前所在章节）即高亮；
//      点击横线 scrollTo 对应章节开头；悬停延迟 150ms 弹出摘要预览卡。
//
// 挂载方式：本组件渲染一个 position:sticky + height:0 的零占位包裹层，
// 作为 .response-area 的第一个子元素插入，横线列 absolute 钉在左缘，
// 跟随滚动视口固定（内容滚动时导航条不动）。
//
// 性能：MutationObserver 在流式输出期间高频触发，refresh 走 rAF 合帧 +
// 签名比对（excerpt+位置），内容没变就不更新 Vue 状态，避免无谓重渲染。
import { ref, computed, watch, onUnmounted } from 'vue';
import { usePreferences } from '../../composables/usePreferences';

interface RailItem {
  el: HTMLElement;
  excerpt: string;
  topPx: number; // 在导航条上的像素位置
}

const props = defineProps<{ container: HTMLElement | null }>();

/* ═══════════════ 调参入口（改完保存即热更新生效） ═══════════════
   ① 块上下间距        → PITCH_MAX / PITCH_MIN（本文件下方，值越大越疏）
   ② 块大小-宽度       → dashWidth() 里的 `26 : 16`（active : 普通）
   ③ 块大小-粗细       → <style> 里 .rail-dash 的 height（active 在 .rail-dash.active）
   ④ 悬停波纹倍率      → HOVER_FACTOR
   ⑤ 整体缩放上限      → railScale 里的 Math.min(1.5, ...)
   ⑥ 与左侧边栏间隔    → <style> 里 .message-rail 的 left: 12px
   ═══════════════════════════════════════════════════════════ */

const RAIL_PAD = 16; // 导航条上下内边距
const PITCH_MAX = 13; // ① 块上下间距上限（px）——整体越疏间隔越大
const PITCH_MIN = 5; // ① 块上下间距下限（px），条目极多时压缩防溢出

const items = ref<RailItem[]>([]);
const activeIndex = ref(-1);
const hoverIndex = ref(-1);
const railHeight = ref(0);
const railPitch = ref(10); // 当前横线间距（px）
const tooNarrow = ref(false);
const prefs = usePreferences();
const preview = ref<{ visible: boolean; x: number; y: number; text: string }>({
  visible: false,
  x: 0,
  y: 0,
  text: '',
});

// 尺寸随界面轻度缩放，两个因子相乘：
// 1) 字号因子：根字号 / 基准 15px —— 设置里放大界面时横线同步轻微变大。
//    注意不能用 px 写死：界面放大走的是 documentElement.style.fontSize，
//    导航条若用固定 px 会成为整个界面里唯一不变大的部分。
// 2) 高度因子：对窗口高度取弱敏感的平方根 —— 高度只带来轻微变化，
//    避免最大化时横线变得臃肿。
const railScale = computed(() => {
  const fontFactor = (prefs.fontSize || 15) / 15;
  const heightFactor = Math.sqrt(railHeight.value / 700);
  return Math.min(1.5, Math.max(0.85, fontFactor * heightFactor)); // ⑤ 整体缩放范围
});

// 悬停邻近联动（Codex 式）：宽度用**基准宽度的倍率**表达而非固定 px——
// 倍率恒定、基准宽随界面缩放，放大窗口/界面字号时绝对伸长量自动等比变大，
// 任何尺寸下波纹的相对幅度都一致。悬停的最长，左右渐短成"一段一段"的轮廓。
const HOVER_FACTOR: Record<number, number> = {
  0: 2.5, // ④ 悬停条：2.0 倍
  1: 1.7, // ④ 相邻：1.7 倍
  2: 1.25, // ④ 再邻：1.25 倍（想波及更多条就往下加 3: 1.1）
};

function dashWidth(i: number): number {
  // active 横线基线略长（宽度统一由 JS 管理）
  const base = (i === activeIndex.value ? 30 : 21) * railScale.value; // ② 块宽度：40=active，25=普通
  const h = hoverIndex.value;
  if (h < 0) return base;
  const d = Math.abs(i - h);
  const factor = HOVER_FACTOR[d];
  return factor ? base * factor : base;
}

let scrollEl: HTMLElement | null = null;
let mutationObserver: MutationObserver | null = null;
let resizeObserver: ResizeObserver | null = null;
let rafId = 0;
let lastSignature = '';
let previewTimer: ReturnType<typeof setTimeout> | null = null;
let hoverResetTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleRefresh() {
  if (rafId) return;
  rafId = requestAnimationFrame(() => {
    rafId = 0;
    refresh();
  });
}

// 摘要：取 .user-text 原文（排除"展开全部"按钮等杂音），预览卡即会话目录
function pickExcerpt(msgEl: HTMLElement): string {
  const textEl = msgEl.querySelector<HTMLElement>('.user-text');
  const text = (textEl?.textContent || '').replace(/\s+/g, ' ').trim();
  return (text || (msgEl.textContent || '').replace(/\s+/g, ' ').trim()).slice(0, 160);
}

function refresh() {
  const root = scrollEl;
  if (!root) return;
  // 只采集 user 消息（章节锚点）：粒度演进 消息+标题 → 消息级 → 仅 user 消息。
  const next: RailItem[] = [];
  root.querySelectorAll<HTMLElement>('.chat-message.user-message').forEach((msgEl) => {
    next.push({ el: msgEl, excerpt: pickExcerpt(msgEl), topPx: 0 });
  });

  // 固定小间距紧凑排下：pitch 夹在 [PITCH_MIN, PITCH_MAX]，整组在导航条内垂直居中；
  // 条目极多撑不下时从顶部开始排
  const n = next.length;
  const innerH = Math.max(railHeight.value - RAIL_PAD * 2, 0);
  // 间距同样随界面缩放
  const pitchMax = PITCH_MAX * railScale.value;
  const pitchMin = PITCH_MIN * railScale.value;
  const pitch = n > 1 ? Math.min(pitchMax, Math.max(pitchMin, innerH / n)) : pitchMax;
  const totalH = n * pitch;
  const start = totalH < innerH ? RAIL_PAD + (innerH - totalH) / 2 : RAIL_PAD;
  railPitch.value = pitch;
  for (let i = 0; i < n; i++) {
    next[i].topPx = start + i * pitch;
  }

  const signature = next.map((i) => i.excerpt + ':' + Math.round(i.topPx)).join('|');
  if (signature !== lastSignature) {
    lastSignature = signature;
    items.value = next;
  }
  updateActive();
}

function updateActive() {
  const root = scrollEl;
  if (!root || items.value.length === 0) {
    activeIndex.value = -1;
    return;
  }
  // 探测线：视口上部约 35% 处（封顶 240px），最后越过它的条目为当前块
  const probe =
    root.getBoundingClientRect().top + Math.min(root.clientHeight * 0.35, 240);
  let idx = 0;
  for (let i = 0; i < items.value.length; i++) {
    if (items.value[i].el.getBoundingClientRect().top <= probe) {
      idx = i;
    } else {
      break;
    }
  }
  activeIndex.value = idx;
}

function onScroll() {
  if (rafId) return;
  rafId = requestAnimationFrame(() => {
    rafId = 0;
    updateActive();
  });
}

function jumpTo(item: RailItem) {
  const root = scrollEl;
  if (!root) return;
  const top =
    item.el.getBoundingClientRect().top -
    root.getBoundingClientRect().top +
    root.scrollTop;
  root.scrollTo({ top: Math.max(0, top - 12), behavior: 'smooth' });
}

function showPreview(item: RailItem, index: number, e: MouseEvent) {
  if (hoverResetTimer) { clearTimeout(hoverResetTimer); hoverResetTimer = null; }
  hoverIndex.value = index;
  if (previewTimer) clearTimeout(previewTimer);
  // currentTarget 在事件派发结束后即置 null，必须先捕获
  const dashEl = e.currentTarget as HTMLElement;
  // 延迟 150ms：鼠标快速滑过横线列时不闪烁
  previewTimer = setTimeout(() => {
    // 卡片水平位置用「预期最终宽度」计算，而不是瞬时 rect——
    // 宽度回弹动画要走 0.25s，若按动画中途的位置定位，横线随后继续伸长的部分
    // 会滑到卡片底下被盖住，视觉上就像"没有变长"。
    const hit = dashEl.getBoundingClientRect();
    preview.value = {
      visible: true,
      x: hit.left + dashWidth(index) + 12,
      y: hit.top + hit.height / 2,
      text: item.excerpt,
    };
  }, 150);
}

function hidePreview() {
  // 延迟 60ms 再清 hoverIndex：相邻横线无缝衔接时，扫动过程中 leave→enter 之间
  // 会瞬间经过 -1，波纹会被反复打断；宽限期内进入新横线则直接接管，波纹连续。
  if (previewTimer) { clearTimeout(previewTimer); previewTimer = null; }
  if (hoverResetTimer) clearTimeout(hoverResetTimer);
  hoverResetTimer = setTimeout(() => {
    hoverIndex.value = -1;
    preview.value = { ...preview.value, visible: false };
  }, 60);
}

function attach(el: HTMLElement) {
  detach();
  scrollEl = el;
  railHeight.value = el.clientHeight;
  tooNarrow.value = el.clientWidth < 720;
  mutationObserver = new MutationObserver(scheduleRefresh);
  mutationObserver.observe(el, { childList: true, subtree: true, characterData: true });
  resizeObserver = new ResizeObserver(() => {
    railHeight.value = el.clientHeight;
    tooNarrow.value = el.clientWidth < 720;
    scheduleRefresh();
  });
  resizeObserver.observe(el);
  el.addEventListener('scroll', onScroll, { passive: true });
  scheduleRefresh();
}

function detach() {
  if (mutationObserver) { mutationObserver.disconnect(); mutationObserver = null; }
  if (resizeObserver) { resizeObserver.disconnect(); resizeObserver = null; }
  if (scrollEl) { scrollEl.removeEventListener('scroll', onScroll); scrollEl = null; }
  if (rafId) { cancelAnimationFrame(rafId); rafId = 0; }
  if (previewTimer) { clearTimeout(previewTimer); previewTimer = null; }
  if (hoverResetTimer) { clearTimeout(hoverResetTimer); hoverResetTimer = null; }
  lastSignature = '';
  items.value = [];
  activeIndex.value = -1;
  preview.value = { visible: false, x: 0, y: 0, text: '' };
}

watch(
  () => props.container,
  (el) => {
    if (el) attach(el);
    else detach();
  },
  { immediate: true, flush: 'post' }
);

onUnmounted(detach);
</script>

<template>
  <div v-show="!tooNarrow && items.length > 1" class="rail-sticky">
    <div class="message-rail" :style="{ height: railHeight + 'px', '--rail-scale': railScale }">
      <div
        v-for="(item, i) in items"
        :key="i"
        class="rail-hit"
        :style="{ top: item.topPx + 'px', height: railPitch + 'px', marginTop: -railPitch / 2 + 'px' }"
        @click.stop="jumpTo(item)"
        @mouseenter="showPreview(item, i, $event)"
        @mouseleave="hidePreview"
      >
        <span
          class="rail-dash"
          :class="{ active: i === activeIndex }"
          :style="{ width: dashWidth(i) + 'px' }"
        ></span>
      </div>
      <Teleport to="body">
        <Transition name="rail-preview-fade">
          <div
            v-if="preview.visible"
            class="rail-preview"
            :style="{ left: preview.x + 'px', top: preview.y + 'px' }"
          >
            {{ preview.text }}
          </div>
        </Transition>
      </Teleport>
    </div>
  </div>
</template>

<style scoped>
/* 零占位 sticky 包裹层：钉在滚动视口顶部，自身不参与布局 */
.rail-sticky {
  position: sticky;
  top: 0;
  height: 0;
  flex: 0 0 auto;
  z-index: 40;
  pointer-events: none;
}

.message-rail {
  position: absolute;
  left: 14px; /* ⑥ 与左侧边栏保持间隔，不贴边 */
  top: 0;
  width: 16px;
  display: block;
}

/* 横线命中区：高度绑定当前间距（上限 10px），只覆盖横线自身附近，
   不会让整排变成一个大按钮 */
.rail-hit {
  position: absolute;
  left: 0;
  width: 18px;
  display: flex;
  align-items: center;
  pointer-events: auto;
  cursor: pointer;
  transition: top 0.15s linear, height 0.15s linear;
}

.rail-dash {
  flex: none; /* 关键：flex 子项默认 flex-shrink:1，会把 width 强行压回命中区宽度，
                 任何悬停伸长都被 flex 吃掉——必须禁用收缩 */
  height: calc(3.5px * var(--rail-scale, 1)); /* ③ 块粗细 */
  border-radius: calc(4.25px * var(--rail-scale, 1));
  background: var(--text-muted);
  opacity: 0.5;
  transform-origin: left center;
  /* 回弹缓动：邻近横线依距离衰减拉长（宽度由 JS 按波纹公式计算），动画交给这里 */
  transition: width 0.25s cubic-bezier(0.34, 1.56, 0.64, 1),
    opacity var(--transition-fast, 0.15s), background var(--transition-fast, 0.15s);
}

.rail-hit:hover .rail-dash {
  opacity: 0.8;
}

.rail-dash.active {
  height: calc(4.5px * var(--rail-scale, 1)); /* ③ active 块粗细 */
  background: var(--text-main);
  opacity: 1;
}

/* 摘要预览卡：Teleport 到 body，fixed 定位（横线命中区坐标即视口坐标） */
.rail-preview {
  position: fixed;
  transform: translateY(-50%);
  max-width: 300px;
  padding: 8px 12px;
  border-radius: var(--radius-lg, 12px);
  background: var(--surface-strong);
  border: 1px solid var(--glass-border);
  box-shadow: var(--shadow-md);
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-soft);
  pointer-events: none;
  z-index: 200;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 4;
  -webkit-box-orient: vertical;
  word-break: break-word;
}

.rail-preview-fade-enter-active {
  transition: opacity 0.12s ease;
}

.rail-preview-fade-enter-from {
  opacity: 0;
}
</style>
