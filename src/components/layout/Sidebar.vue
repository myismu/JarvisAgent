<!--
# Sidebar.vue — 应用侧边栏与会话列表

展示项目/会话分组导航、状态灯、归档计数与行内重命名交互；业务逻辑已下沉至 `useSessionSidebar`。

## Key Exports
- `Sidebar`: 左侧导航与会话管理组件

## Dependencies
- Internal: `@/composables/sidebar/useSessionSidebar`
-->
<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { useSessionSidebar } from '../../composables/sidebar/useSessionSidebar';

defineProps<{
  collapsed: boolean;
}>();

const emit = defineEmits<{
  (e: 'open-settings'): void;
}>();

const { t } = useI18n();

const {
  sessionStore,
  appView,
  projects,
  sessions,
  archivedCount,
  collapsedProjects,
  projectsSectionCollapsed,
  standaloneCollapsed,
  editingProjectId,
  editingProjectName,
  editingSessionId,
  editingTitle,
  standaloneSessions,
  projectSessions,
  sessionLightStates,
  confirmingDeleteProjectId,
  toggleProject,
  formatSessionTime,
  formatSessionTokens,
  openArchive,
  openExtensions,
  startRenameSession,
  cancelRenameSession,
  submitRenameSession,
  startRenameProject,
  cancelRenameProject,
  submitRenameProject,
  createNewSession,
  openProject,
  createProjectSession,
  deleteProject,
  switchToSession,
  deleteSession,
} = useSessionSidebar();
</script>

<template>
  <div class="sidebar" :class="{ collapsed }">
    <div v-if="!collapsed" class="sidebar-content">
      <div class="sidebar-main">
      <div class="sidebar-section">
        <div class="session-btn-group">
          <button type="button" class="new-session-btn primary-new-btn" @click.stop="createNewSession()">
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none">
              <line x1="12" y1="5" x2="12" y2="19"></line>
              <line x1="5" y1="12" x2="19" y2="12"></line>
            </svg>
            <span>{{ t('sidebar.newSession') }}</span>
          </button>
          <button type="button" class="new-session-btn project-btn" @click.stop="openProject()">
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
            </svg>
            <span>{{ t('sidebar.openProject') }}</span>
          </button>
          <button
            type="button"
            class="new-session-btn skill-btn"
            :class="{ active: appView.isExtensionsView }"
            @click.stop="openExtensions()"
          >
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="7" height="7" rx="1.5"></rect>
              <rect x="3" y="14" width="7" height="7" rx="1.5"></rect>
              <rect x="14" y="14" width="7" height="7" rx="1.5"></rect>
              <rect x="14" y="3" width="7" height="7" rx="1.5" transform="rotate(12 17.5 6.5)"></rect>
            </svg>
            <span>{{ t('sidebar.extensions') }}</span>
          </button>
          <!-- 归档入口：垃圾桶图标 + 存量徽标。归档页集中承担列表 / 恢复 / 彻底删除，
               所以侧边栏这里不再有折叠块，也不再有任何彻底删除入口 —— 销毁性操作
               必须离开主界面，避免用户在图方便时误点。 -->
          <button
            type="button"
            class="new-session-btn archive-btn"
            :class="{ active: appView.isArchiveView }"
            :title="t('sidebar.recentlyDeleted')"
            @click.stop="openArchive()"
          >
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <rect width="20" height="5" x="2" y="3" rx="1"></rect>
              <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"></path>
              <path d="M10 12h4"></path>
            </svg>
            <span>{{ t('sidebar.recentlyDeleted') }}</span>
            <span v-if="archivedCount > 0" class="archive-count">{{ archivedCount }}</span>
          </button>
        </div>
        <div v-if="sessions.length === 0 && projects.length === 0" class="session-empty-state">
          {{ t('sidebar.noSessions') }}
        </div>

        <!-- 项目分组 -->
        <!-- 项目总区块：一次折叠全部项目分组（不含独立会话） -->
        <div
          v-if="projects.length > 0"
          class="project-header projects-section-header"
          @click="projectsSectionCollapsed = !projectsSectionCollapsed"
        >
          <svg
            viewBox="0 0 24 24" width="8" height="8" stroke="currentColor" stroke-width="2.5" fill="none"
            class="project-chevron"
            :class="{ collapsed: projectsSectionCollapsed }"
          >
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" class="project-icon">
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
          </svg>
          <span class="project-name">{{ t('sidebar.projectsSection') }}</span>
          <span class="project-count">{{ projects.length }}</span>
        </div>

        <template v-if="!projectsSectionCollapsed">
        <template v-for="group in projectSessions" :key="group.project.id">
          <div class="project-header project-group-header" @click="toggleProject(group.project.id)">
            <svg
              viewBox="0 0 24 24" width="8" height="8" stroke="currentColor" stroke-width="2.5" fill="none"
              class="project-chevron"
              :class="{ collapsed: collapsedProjects.has(group.project.id) }"
            >
              <polyline points="9 18 15 12 9 6"></polyline>
            </svg>
            <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" class="project-icon">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
            </svg>
            <input v-if="editingProjectId === group.project.id" v-model="editingProjectName" class="session-title-input" maxlength="40" @click.stop @keydown.enter.prevent="submitRenameProject(group.project.id)" @keydown.esc.prevent="cancelRenameProject" @blur="submitRenameProject(group.project.id)" />
            <span v-else class="project-name">{{ group.project.name }}</span>
            <span class="project-count">{{ group.sessions.length }}</span>
            <button class="project-rename-btn" @click.stop="startRenameProject(group.project, $event)" :title="t('sidebar.renameProject')">
              <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                <path d="M12 20h9"></path>
                <path d="M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z"></path>
              </svg>
            </button>
            <button class="project-new-btn" @click.stop="createProjectSession(group.project.id)" :title="t('sidebar.newProjectSession')">
              <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none">
                <line x1="12" y1="5" x2="12" y2="19"></line>
                <line x1="5" y1="12" x2="19" y2="12"></line>
              </svg>
            </button>
            <button
              class="project-delete-btn"
              :class="{ confirming: confirmingDeleteProjectId === group.project.id }"
              @click="deleteProject(group.project.id, $event)"
              :title="confirmingDeleteProjectId === group.project.id ? t('common.confirm') : t('sidebar.deleteProjectHint')"
            >
              <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
              <span v-if="confirmingDeleteProjectId === group.project.id" class="confirm-label">{{ t('common.confirm') }}</span>
            </button>
          </div>
          <Transition name="project-expand">
            <ul v-if="!collapsedProjects.has(group.project.id)" class="session-list project-session-list">
              <li
                v-for="session in group.sessions"
              :key="session.id"
              :class="['session-item', { active: appView.isChatView && session.id === sessionStore.activeSessionId }]"
              @click="editingSessionId !== session.id && switchToSession(session.id)"
            >
              <div class="session-main-row">
                <span
                v-if="sessionLightStates[session.id]"
                class="session-state-light state-light"
                :class="`is-${sessionLightStates[session.id]}`"
              ></span>
                <input
                  v-if="editingSessionId === session.id"
                  v-model="editingTitle"
                  class="session-title-input"
                  maxlength="30"
                  @click.stop
                  @keydown.enter.prevent="submitRenameSession(session.id)"
                  @keydown.esc.prevent="cancelRenameSession"
                  @blur="submitRenameSession(session.id)"
                />
                <span v-else class="session-title">{{ session.title }}</span>
                                <button class="rename-btn" @click="startRenameSession(session, $event)" :title="t('sidebar.rename')">
                  <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                    <path d="M12 20h9"></path>
                    <path d="M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z"></path>
                  </svg>
                </button>
                <button
                  class="delete-btn"
                  @click="deleteSession(session, $event)"
                  :title="t('sidebar.delete')"
                >
                  <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                    <rect width="20" height="5" x="2" y="3" rx="1"></rect>
                    <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"></path>
                    <path d="M10 12h4"></path>
                  </svg>
                </button>
              </div>
              <div class="session-meta-row">
                <span>{{ formatSessionTime(session.updatedAt) }}</span>
                <span v-if="formatSessionTokens(session)" class="session-tokens">{{ formatSessionTokens(session) }}</span>
              </div>
            </li>
          </ul>
          </Transition>
        </template>
        </template>

        <!-- 独立会话 -->
        <div class="project-header projects-section-header standalone-header" @click="standaloneCollapsed = !standaloneCollapsed">
          <svg viewBox="0 0 24 24" width="8" height="8" stroke="currentColor" stroke-width="2.5" fill="none" class="project-chevron" :class="{ collapsed: standaloneCollapsed }">
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
          <svg viewBox="0 0 24 24" width="13" height="13" stroke="currentColor" stroke-width="2" fill="none" class="project-icon">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
          </svg>
          <span class="project-name">{{ t('sidebar.standaloneSessions') }}</span>
          <span class="project-count">{{ standaloneSessions.length }}</span>
        </div>
        <ul v-if="!standaloneCollapsed && standaloneSessions.length > 0" class="session-list">
          <li
            v-for="session in standaloneSessions"
            :key="session.id"
            :class="['session-item', { active: appView.isChatView && session.id === sessionStore.activeSessionId }]"
            @click="editingSessionId !== session.id && switchToSession(session.id)"
          >
            <div class="session-main-row">
              <span
                v-if="sessionLightStates[session.id]"
                class="session-state-light state-light"
                :class="`is-${sessionLightStates[session.id]}`"
              ></span>
              <input
                v-if="editingSessionId === session.id"
                v-model="editingTitle"
                class="session-title-input"
                maxlength="30"
                @click.stop
                @keydown.enter.prevent="submitRenameSession(session.id)"
                @keydown.esc.prevent="cancelRenameSession"
                @blur="submitRenameSession(session.id)"
              />
              <span v-else class="session-title">{{ session.title }}</span>
                            <button class="rename-btn" @click="startRenameSession(session, $event)" :title="t('sidebar.rename')">
                <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none">
                  <path d="M12 20h9"></path>
                  <path d="M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4 12.5-12.5z"></path>
                </svg>
              </button>
              <button
                class="delete-btn"
                @click="deleteSession(session, $event)"
                :title="t('sidebar.delete')"
              >
                <svg viewBox="0 0 24 24" width="11" height="11" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                  <rect width="20" height="5" x="2" y="3" rx="1"></rect>
                  <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"></path>
                  <path d="M10 12h4"></path>
                </svg>
              </button>
            </div>
            <div class="session-meta-row">
              <span>{{ formatSessionTime(session.updatedAt) }}</span>
              <span v-if="formatSessionTokens(session)" class="session-tokens">{{ formatSessionTokens(session) }}</span>
            </div>
          </li>
        </ul>
      </div>
      </div>

      <div class="sidebar-footer">
        <button type="button" class="footer-action" @click="emit('open-settings')" :title="t('sidebar.settingsTitle')">
          <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3"></circle>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
          </svg>
          <span>{{ t('sidebar.settings') }}</span>
        </button>
      </div>
    </div>
  </div>

</template>

<style scoped>
.sidebar {
  width: 250px;
  background: var(--bg-sidebar);
  border-right: 1px solid var(--glass-border-subtle);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 10px 8px;
  transition: width 0.25s ease, padding 0.25s ease;
  flex-shrink: 0;
}

.sidebar.collapsed {
  width: 0;
  padding: 0;
  border-right: none;
  overflow: hidden;
}

.sidebar-content {
  min-width: 234px;
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.sidebar-main {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding-right: 2px;
}

.sidebar-main::-webkit-scrollbar {
  width: 6px;
}

.sidebar-main::-webkit-scrollbar-thumb {
  background: var(--glass-border);
  border-radius: 999px;
}

.sidebar-footer {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  padding: 10px 4px 0;
  border-top: 1px solid var(--glass-border-subtle);
}

.footer-action {
  min-width: 0;
  height: 32px;
  flex: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border-radius: var(--radius-md);
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.78rem;
  font-weight: 500;
  transition: all var(--transition-fast);
  -webkit-app-region: no-drag;
}

.footer-action.icon-only {
  flex: 0 0 34px;
}

.footer-action:hover {
  color: var(--text-main);
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
}

.sidebar-section {
  display: flex;
  flex-direction: column;
  margin-bottom: 16px;
}

.sidebar-section + .sidebar-section {
  border-top: 1px solid var(--glass-border-subtle);
  padding-top: 16px;
}

.sidebar-title {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-soft);
  padding: 4px 12px 8px;
  letter-spacing: 0.05em;
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-shrink: 0;
  text-transform: uppercase;
}

.session-btn-group {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 2px 4px 12px;
}

.new-session-btn {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-soft);
  cursor: pointer;
  padding: 7px 10px;
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  border-radius: var(--radius-md);
  font-size: 0.84rem;
  font-weight: 500;
  transition: background var(--transition-fast), border-color var(--transition-fast), color var(--transition-fast), box-shadow var(--transition-fast);
  -webkit-app-region: no-drag;
  text-align: left;
}

/* 主入口「新建对话」：在浅灰侧栏上以微抬升白卡突出层级，其余导航项保持无边框轻盈感 */
.primary-new-btn {
  background: var(--bg-panel);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
  font-weight: 600;
  margin-bottom: 6px;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.03);
}

.primary-new-btn:hover {
  background: var(--bg-panel);
  border-color: var(--glass-border);
  box-shadow: var(--shadow-sm);
}

.new-session-btn span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.new-session-btn:not(.primary-new-btn):hover,
.project-btn:hover {
  background: var(--surface-1);
  border-color: transparent;
  color: var(--text-main);
}

/* 视图入口按钮（技能管理 / 归档）的选中态 = “当前处于该视图”，样式与聊天视图下会话项的选中态同源，
   两者互斥由内容区视图决定：进入技能视图 / 归档页时聊天视图的会话高亮前提不再成立 */
.skill-btn.active,
.archive-btn.active {
  background: var(--bg-panel);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  box-shadow: var(--shadow-sm);
}

/* 归档入口的数量徽标：弱化处理——虚线边 + 弱字色，读作“不活跃存量”，
   不与按钮文字抢视线；数量为 0 时模板直接不渲染 */
.archive-count {
  margin-left: auto;
  flex-shrink: 0;
  padding: 1px 6px;
  font-size: 0.65rem;
  font-family: var(--font-mono);
  border-radius: 10px;
  border: 1px dashed var(--glass-border-subtle);
  color: var(--text-faint);
}

.project-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 8px 12px 8px 0;
  margin: 6px 0 2px;
  border-radius: var(--radius-md);
  cursor: pointer;
  color: var(--text-muted);
  font-size: 0.85rem;
  font-weight: 600;
  transition: all var(--transition-fast);
  border: 1px solid transparent;
}

body.dark-mode .project-header {
  color: rgba(255, 255, 255, 0.7);
}

.standalone-header {
  cursor: pointer;
}

.project-header:not(.standalone-header):hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
}

.project-chevron {
  flex-shrink: 0;
  transition: transform 0.2s ease;
}

.project-chevron.collapsed {
  transform: rotate(0deg);
}

.project-chevron:not(.collapsed) {
  transform: rotate(90deg);
}

.project-icon {
  flex-shrink: 0;
  opacity: 0.85;
}

.project-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-count {
  font-size: 0.65rem;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  padding: 1px 6px;
  border-radius: 10px;
  border: 1px solid var(--glass-border-subtle);
}

.project-rename-btn,
.project-new-btn,
.project-delete-btn {
  display: none;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px;
  border-radius: var(--radius-md);
  align-items: center;
  justify-content: center;
  transition: all var(--transition-fast);
}

.project-header:not(.standalone-header):hover .project-rename-btn,
.project-header:not(.standalone-header):hover .project-new-btn,
.project-header:not(.standalone-header):hover .project-delete-btn {
  display: inline-flex;
}

.project-delete-btn.confirming {
  display: inline-flex;
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 12%, transparent);
  border-color: color-mix(in srgb, var(--accent-red) 25%, transparent);
  gap: 3px;
  padding: 1px 6px;
  border-radius: var(--radius-md);
  font-size: 0.65rem;
  font-weight: 600;
  line-height: 1;
  height: 17px;
  box-sizing: border-box;
}

.project-delete-btn.confirming:hover {
  background: color-mix(in srgb, var(--accent-red) 20%, transparent);
  border-color: color-mix(in srgb, var(--accent-red) 35%, transparent);
}

.confirm-label {
  white-space: nowrap;
  line-height: 1;
}

.project-new-btn:hover,
.project-delete-btn:not(.confirming):hover {
  color: var(--text-main);
  background: color-mix(in srgb, var(--text-muted) 14%, transparent);
  border-color: color-mix(in srgb, var(--text-muted) 22%, transparent);
}

.project-session-list {
  margin-left: 20px;
  border-left: 1px solid var(--glass-border-subtle);
  padding-left: 12px;
}

.project-expand-enter-active,
.project-expand-leave-active {
  transition: opacity 0.3s ease, transform 0.3s ease;
  transform-origin: top;
}
.project-expand-enter-from,
.project-expand-leave-to {
  opacity: 0;
  transform: translateY(-10px);
}

.project-new-session-item {
  padding: 4px 12px;
}

.project-new-session-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 0;
  border: 1px dashed transparent;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.78rem;
  transition: all var(--transition-fast);
}

.project-new-session-btn:hover {
  color: var(--text-main);
  border-color: var(--glass-border-subtle);
}

.standalone-header {
  cursor: pointer;
}

/* ── 会话列表层级 ─────────────────────────────────────────────
   三层：分区标题（项目 / 独立会话 / 最近删除）> 项目分组 > 会话。
   靠字号、颜色强度与缩进区分——侧边栏宽度有限，层级不清时用户一眼分不出
   “哪个是可折叠的分区、哪个是项目”。
   分区标题用弱色小字 + 底部分隔线，视觉上“切段”而不是“条目”；
   项目分组保持可点击条目的 hover 反馈，但缩进一级。 */
.project-header.projects-section-header {
  margin: 12px 0 2px;
  padding: 6px 8px 6px 2px;
  border: none;
  border-bottom: 1px solid var(--glass-border-subtle);
  border-radius: 0;
  font-size: 0.72rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--text-faint);
}

/* 分区标题 hover 只提亮颜色，不加底色——它不可选中，只是分类标识 */
.project-header.projects-section-header:hover {
  background: transparent;
  border-color: transparent;
  border-bottom-color: var(--glass-border);
  color: var(--text-muted);
}

.project-header.projects-section-header .project-icon {
  width: 11px;
  height: 11px;
  opacity: 0.55;
}

.project-header.project-group-header {
  padding-left: 10px;
  font-size: 0.82rem;
  font-weight: 500;
}

body.dark-mode .project-header.projects-section-header {
  color: rgba(255, 255, 255, 0.42);
}

body.dark-mode .project-header.projects-section-header:hover {
  color: rgba(255, 255, 255, 0.68);
}

/* 项目区总折叠头：一次收起全部项目分组，独立会话不受影响 */
.projects-section-header {
  cursor: pointer;
}

.session-empty-state {
  margin: 4px 12px 8px;
  padding: 10px;
  color: var(--text-muted);
  border: 1px dashed var(--glass-border-subtle);
  border-radius: var(--radius-md);
  font-size: 0.74rem;
  text-align: center;
}

.session-list {
  list-style: none;
  padding: 0 0 0 12px;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.session-item {
  padding: 6px 14px 6px 26px;
  font-size: 0.78rem;
  font-weight: 400;
  border-radius: var(--radius-md);
  display: flex;
  flex-direction: column;
  gap: 2px;
  align-items: stretch;
  cursor: pointer;
  color: var(--text-muted);
  transition: all var(--transition-fast);
  position: relative;
  background: transparent;
  border: 1px solid transparent;
}

.session-main-row {
  display: flex;
  align-items: center;
  min-width: 0;
  min-height: 24px;
}

.session-meta-row {
  display: flex;
  gap: 8px;
  padding-left: 0;
  /* 元信息整行再降一档（faint）：让标题在列表里绝对主导，时间/token 只作余光信息 */
  color: var(--text-faint);
  font-size: 0.62rem;
  font-weight: 400;
  white-space: nowrap;
  overflow: hidden;
}

.session-meta-row span {
  overflow: hidden;
  text-overflow: ellipsis;
}

/* token 读数：等宽数字 + 再弱半档，与文字形的时间区分开 */
.session-meta-row .session-tokens {
  font-family: var(--font-mono);
  opacity: 0.75;
}
.session-item:hover {
  background: var(--glass-bg-light);
  border-color: var(--glass-border-subtle);
  color: var(--text-main);
}
.session-item.active {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  color: var(--text-main);
  font-weight: 600;
  box-shadow: var(--shadow-sm);
}

body.dark-mode .session-item.active {
  color: rgba(255, 255, 255, 0.95);
}

/* 会话行的状态灯：颜色与呼吸节奏全部由全局 .state-light 决定，
   这里只管尺寸与间距。6px 比默认 7px 小一档 —— 它紧排在标题文字前，
   再大会抢视线；侧边栏的信息主体是标题，不是这颗灯。 */
.session-state-light {
  --light-size: 6px;
  margin-right: 4px;
}

.session-title {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-title-input {
  flex: 1;
  min-width: 0;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  color: var(--text-main);
  border-radius: var(--radius-sm);
  padding: 4px 8px;
  font-size: 0.82rem;
}

.session-title-input:focus {
  outline: none;
  border-color: var(--accent-blue);
}

.delete-btn {
  display: none;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  margin-left: 4px;
  border-radius: var(--radius-md);
  align-items: center;
  justify-content: center;
  -webkit-app-region: no-drag;
  transition: all var(--transition-fast);
}
.rename-btn {
  display: none;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-soft);
  cursor: pointer;
  padding: 4px;
  margin-left: 4px;
  border-radius: var(--radius-md);
  align-items: center;
  justify-content: center;
  -webkit-app-region: no-drag;
  transition: all var(--transition-fast);
}
.session-item:hover .delete-btn,
.session-item:hover .rename-btn {
  display: inline-flex;
  animation: fadeIn var(--transition-fast);
}
@keyframes fadeIn {
  from { opacity: 0; transform: scale(0.9); }
  to { opacity: 1; transform: scale(1); }
}
.delete-btn:hover {
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 15%, transparent);
  border-color: color-mix(in srgb, var(--accent-red) 30%, transparent);
}
.rename-btn:hover {
  color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 15%, transparent);
  border-color: color-mix(in srgb, var(--accent-blue) 30%, transparent);
}

@keyframes spin {
  100% { transform: rotate(360deg); }
}
.spin {
  animation: spin 2s linear infinite;
}
</style>
