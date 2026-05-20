//! # appView.ts — 应用视图状态管理
//!
//! 管理主内容区的视图切换（聊天视图 vs 技能管理视图）。
//! 使用独立 store 而非扩展 session store，因为视图切换与会话管理是正交关注点。
//!
//! ## 关键导出
//! - `useAppViewStore`: Pinia store 实例
//! - `activeView`: 当前活跃视图
//! - `showChat()` / `showSkillManager()`: 视图切换方法
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

  return {
    activeView,
    isChatView,
    isSkillManagerView,
    setView,
    showChat,
    showSkillManager,
    toggleSkillManager,
  };
});
