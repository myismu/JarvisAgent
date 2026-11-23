//! # appView.ts — 应用视图状态管理
//!
//! 管理主内容区的视图切换（聊天视图 / 技能管理视图 / 归档管理视图）。
//! 使用独立 store 而非扩展 session store，因为视图切换与会话管理是正交关注点。
//!
//! ## 关键导出
//! - `useAppViewStore`: Pinia store 实例
//! - `activeView`: 当前活跃视图
//! - `showChat()` / `showSkillManager()` / `showArchive()` / `toggleArchive()`: 视图切换方法
//!
//! ## 依赖
//! - Internal: `../types`
//! - External: `pinia`

import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type { AppView } from "../types";

export const useAppViewStore = defineStore("appView", () => {
  const activeView = ref<AppView>("chat");

  const isChatView = computed(() => activeView.value === "chat");
  const isSkillManagerView = computed(() => activeView.value === "skill-manager");
  // 归档管理视图（列出已归档会话，支持恢复与彻底删除）
  const isArchiveView = computed(() => activeView.value === "archive");

  function setView(view: AppView) {
    activeView.value = view;
  }

  function showChat() {
    activeView.value = "chat";
  }

  function showSkillManager() {
    activeView.value = "skill-manager";
  }

  function toggleSkillManager() {
    activeView.value = activeView.value === "skill-manager" ? "chat" : "skill-manager";
  }

  function showArchive() {
    activeView.value = "archive";
  }

  /** 再次点击归档入口时回到对话视图，与技能管理的开关语义保持一致 */
  function toggleArchive() {
    activeView.value = activeView.value === "archive" ? "chat" : "archive";
  }

  return {
    activeView,
    isChatView,
    isSkillManagerView,
    isArchiveView,
    setView,
    showChat,
    showSkillManager,
    toggleSkillManager,
    showArchive,
    toggleArchive,
  };
});
