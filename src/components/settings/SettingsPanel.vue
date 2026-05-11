<template>
  <div v-if="modelValue" class="settings-overlay" @click.self="close">
    <div class="settings-modal">
      <div class="settings-header">
        <div class="header-title">
          <svg viewBox="0 0 24 24" width="20" height="20" class="header-icon">
            <path fill="currentColor" d="M12,15.5A3.5,3.5 0 0,1 8.5,12A3.5,3.5 0 0,1 12,8.5A3.5,3.5 0 0,1 15.5,12A3.5,3.5 0 0,1 12,15.5M19.43,12.97C19.47,12.65 19.5,12.33 19.5,12C19.5,11.67 19.47,11.34 19.43,11.03L21.54,9.37C21.73,9.22 21.78,8.95 21.66,8.73L19.66,5.27C19.54,5.05 19.27,4.96 19.05,5.05L16.56,6.05C16.04,5.66 15.47,5.34 14.86,5.08L14.47,2.44C14.43,2.21 14.23,2 14,2H10C9.77,2 9.57,2.21 9.53,2.44L9.14,5.08C8.53,5.34 7.96,5.66 7.44,6.05L4.95,5.05C4.73,4.96 4.46,5.05 4.34,5.27L2.34,8.73C2.21,8.95 2.27,9.22 2.46,9.37L4.57,11.03C4.53,11.34 4.5,11.67 4.5,12C4.5,12.33 4.53,12.65 4.57,12.97L2.46,14.63C2.27,14.78 2.21,15.05 2.34,15.27L4.34,18.73C4.46,18.95 4.73,19.03 4.95,18.95L7.44,17.94C7.96,18.34 8.53,18.66 9.14,18.92L9.53,21.56C9.57,21.79 9.77,22 10,22H14C14.23,22 14.43,21.79 14.47,21.56L14.86,18.92C15.47,18.66 16.04,18.34 16.56,17.94L19.05,18.95C19.27,19.03 19.54,18.95 19.66,18.73L21.66,15.27C21.78,15.05 21.73,14.78 21.54,14.63L19.43,12.97Z" />
          </svg>
          <h3>{{ t('settings.title') }}</h3>
        </div>
        <button class="icon-btn close-btn" @click="close" :title="t('settings.close')">
          <svg viewBox="0 0 24 24" width="20" height="20">
            <path fill="currentColor" d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z" />
          </svg>
        </button>
      </div>

      <div class="settings-container">
        <!-- 左侧导航 -->
        <div class="settings-sidebar">
          <div class="sidebar-nav">
            <div
              class="nav-item"
              :class="{ active: activeTab === 'general' }"
              @click="activeTab = 'general'"
            >
              <svg viewBox="0 0 24 24" width="18" height="18">
                <path fill="currentColor" d="M12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6A6,6 0 0,1 18,12A6,6 0 0,1 12,18M12,8A4,4 0 0,0 8,12A4,4 0 0,0 12,16A4,4 0 0,0 16,12A4,4 0 0,0 12,8M12,2L4.5,20.29L5.21,21L12,18L18.79,21L19.5,20.29L12,2Z" />
              </svg>
              <span>{{ t('settings.tabs.general') }}</span>
            </div>
            <div
              class="nav-item"
              :class="{ active: activeTab === 'presets' }"
              @click="activeTab = 'presets'"
            >
              <svg viewBox="0 0 24 24" width="18" height="18">
                <path fill="currentColor" d="M13,9.5H11V7.5H13V9.5M13,16.5H11V11.5H13V16.5M12,2A10,10 0 0,1 22,12A10,10 0 0,1 12,22A10,10 0 0,1 2,12A10,10 0 0,1 12,2M12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20A8,8 0 0,0 20,12A8,8 0 0,0 12,4Z" />
              </svg>
              <span>{{ t('settings.tabs.presets') }}</span>
            </div>
          </div>

          <template v-if="activeTab === 'presets'">
            <div class="sidebar-divider"></div>
            <div class="sidebar-section-header">
              <span>{{ t('settings.profiles.section') }}</span>
              <button class="add-btn" @click="addProfile" :title="t('settings.profiles.add')">+</button>
            </div>
            <div class="profile-list">
              <div
                v-for="(profile, index) in draftConfig.profiles"
                :key="profile.id"
                class="profile-item"
                :class="{
                  'active': selectedProfileId === profile.id,
                  'is-running': draftConfig.activeProfileId === profile.id,
                  'drag-over': dragIndex !== null && dragOverIndex === index && dragIndex !== index,
                  'dragging': dragIndex === index && dragMoved
                }"
                :style="dragIndex === index && dragMoved ? { transform: `translateY(${dragOffsetY}px)`, zIndex: 10 } : {}"
                @mousedown="onMouseDown($event, index)"
                @mousemove="onMouseMove($event)"
                @mouseup="onMouseUp($event, index)"
              >
                <span class="profile-name">{{ profile.name }}</span>
                <div class="profile-actions">
                  <label class="sidebar-switch" :title="t('settings.profiles.setGlobal')" @click.stop>
                    <input
                      type="checkbox"
                      :checked="savedConfig.globalProfileId === profile.id"
                      :disabled="actionLoading"
                      @change="toggleGlobalProfile(profile.id)"
                    />
                    <span class="slider"></span>
                  </label>
                  <button
                    class="copy-btn"
                    :disabled="actionLoading"
                    @click.stop="copyProfile(profile.id)"
                    :title="t('settings.profiles.copy')"
                  >
                    <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                  </button>
                  <button
                    v-if="draftConfig.profiles.length > 1"
                    class="delete-btn"
                    :disabled="actionLoading"
                    @click.stop="requestDeleteProfile(profile.id)"
                  >
                    <svg viewBox="0 0 24 24" width="14" height="14"><path fill="currentColor" d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>
                  </button>
                </div>
              </div>
            </div>
          </template>
        </div>

        <!-- 右侧内容区域 -->
        <div class="settings-content">
          <div class="settings-body">
            <!-- 常规设置页 -->
            <div v-if="activeTab === 'general'" class="tab-content">
              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,18V6L5,12L12,18M11,14.14L11,9.86L8.5,12L11,14.14M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M12,20A8,8 0 0,1 4,12A8,8 0 0,1 12,4A8,8 0 0,1 20,12A8,8 0 0,1 12,20Z"/></svg>
                  <h4>{{ t('settings.general.appearance') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.colorMode') }}</label>
                  <button class="theme-toggle-btn" @click="toggleTheme">
                    <svg v-if="isDark" viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                      <circle cx="12" cy="12" r="5"></circle>
                      <line x1="12" y1="1" x2="12" y2="3"></line>
                      <line x1="12" y1="21" x2="12" y2="23"></line>
                      <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"></line>
                      <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"></line>
                      <line x1="1" y1="12" x2="3" y2="12"></line>
                      <line x1="21" y1="12" x2="23" y2="12"></line>
                      <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"></line>
                      <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"></line>
                    </svg>
                    <svg v-else viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"></path>
                    </svg>
                    <span>{{ isDark ? t('settings.general.darkMode') : t('settings.general.lightMode') }}</span>
                  </button>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.fontSize') }}</label>
                  <div class="font-size-control">
                    <button class="font-size-step" @click="setFontSize(Math.max(11, fontSize - 1))" :disabled="fontSize <= 11" :title="t('settings.general.zoomOut')">
                      <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/><line x1="8" y1="11" x2="14" y2="11"/></svg>
                    </button>
                    <div class="slider-track-wrap">
                      <input
                        type="range"
                        min="11"
                        max="22"
                        :value="fontSize"
                        class="font-size-slider"
                        :style="{ '--fill-pct': ((fontSize - 11) / (22 - 11) * 100).toFixed(0) + '%' }"
                        @input="setFontSize(Number(($event.target as HTMLInputElement).value))"
                      />
                    </div>
                    <button class="font-size-step" @click="setFontSize(Math.min(22, fontSize + 1))" :disabled="fontSize >= 22" :title="t('settings.general.zoomIn')">
                      <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/><line x1="8" y1="11" x2="14" y2="11"/><line x1="11" y1="8" x2="11" y2="14"/></svg>
                    </button>
                    <span class="font-size-value">{{ fontSize }}px</span>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.fontSizeDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.codeFontSize') }}</label>
                  <div class="font-size-control">
                    <button class="font-size-step" @click="setCodeFontSize(Math.max(10, codeFontSize - 1))" :disabled="codeFontSize <= 10" :title="t('settings.general.zoomOut')">
                      <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/><line x1="8" y1="11" x2="14" y2="11"/></svg>
                    </button>
                    <div class="slider-track-wrap">
                      <input
                        type="range"
                        min="10"
                        max="20"
                        :value="codeFontSize"
                        class="font-size-slider"
                        :style="{ '--fill-pct': ((codeFontSize - 10) / (20 - 10) * 100).toFixed(0) + '%' }"
                        @input="setCodeFontSize(Number(($event.target as HTMLInputElement).value))"
                      />
                    </div>
                    <button class="font-size-step" @click="setCodeFontSize(Math.min(20, codeFontSize + 1))" :disabled="codeFontSize >= 20" :title="t('settings.general.zoomIn')">
                      <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/><line x1="8" y1="11" x2="14" y2="11"/><line x1="11" y1="8" x2="11" y2="14"/></svg>
                    </button>
                    <span class="font-size-value">{{ codeFontSize }}px</span>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.codeFontSizeDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.language') }}</label>
                  <div class="custom-select" :class="{ open: langMenuOpen }">
                    <button class="custom-select-trigger" @click="langMenuOpen = !langMenuOpen">
                      <span>{{ localeOptions[appLocale] }}</span>
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
                    </button>
                    <div v-if="langMenuOpen" class="custom-select-menu">
                      <div
                        v-for="(label, value) in localeOptions"
                        :key="value"
                        class="custom-select-option"
                        :class="{ active: appLocale === value }"
                        @click="setAppLocale(value); langMenuOpen = false"
                      >{{ label }}</div>
                    </div>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.languageDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.audience') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentAudience === 'user' }"
                      @click="setAgentAudience('user')"
                    >{{ t('settings.general.normal') }}</button>
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentAudience === 'developer' }"
                      @click="setAgentAudience('developer')"
                    >{{ t('settings.general.developer') }}</button>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.audienceDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.workMode') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentWorkMode === 'chat' }"
                      @click="setAgentWorkMode('chat')"
                    >{{ t('settings.general.chat') }}</button>
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentWorkMode === 'edit' }"
                      @click="setAgentWorkMode('edit')"
                    >{{ t('settings.general.edit') }}</button>
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentWorkMode === 'plan' }"
                      @click="setAgentWorkMode('plan')"
                    >{{ t('settings.general.plan') }}</button>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.workModeDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.compactMode') }}</label>
                  <label class="toggle-switch">
                    <input type="checkbox" :checked="compactMode" @change="setCompactMode(($event.target as HTMLInputElement).checked)" />
                    <span class="toggle-slider"></span>
                  </label>
                  <div class="setting-desc">{{ t('settings.general.compactModeDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.agentMessageOpacity') }}</label>
                  <div class="font-size-control">
                    <div class="slider-track-wrap">
                      <input type="range" min="0" max="100" :value="agentMessageOpacity" class="font-size-slider"
                        :style="{ '--fill-pct': agentMessageOpacity + '%' }"
                        @input="setAgentMessageOpacity(Number(($event.target as HTMLInputElement).value))" />
                    </div>
                    <span class="font-size-value">{{ agentMessageOpacity }}%</span>
                  </div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.userMessageOpacity') }}</label>
                  <div class="font-size-control">
                    <div class="slider-track-wrap">
                      <input type="range" min="0" max="100" :value="userMessageOpacity" class="font-size-slider"
                        :style="{ '--fill-pct': userMessageOpacity + '%' }"
                        @input="setUserMessageOpacity(Number(($event.target as HTMLInputElement).value))" />
                    </div>
                    <span class="font-size-value">{{ userMessageOpacity }}%</span>
                  </div>
                </div>
              </div>

              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M12,4A8,8 0 0,1 20,12A8,8 0 0,1 12,20A8,8 0 0,1 4,12A8,8 0 0,1 12,4M12,17A5,5 0 0,1 7,12A5,5 0 0,1 12,7A5,5 0 0,1 17,12A5,5 0 0,1 12,17Z"/></svg>
                  <h4>{{ t('settings.general.behavior') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.autoScroll') }}</label>
                  <label class="toggle-switch">
                    <input type="checkbox" :checked="autoScroll" @change="setAutoScroll(($event.target as HTMLInputElement).checked)" />
                    <span class="toggle-slider"></span>
                  </label>
                  <div class="setting-desc">{{ t('settings.general.autoScrollDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.defaultExpandThinking') }}</label>
                  <label class="toggle-switch">
                    <input type="checkbox" :checked="defaultExpandThinking" @change="setDefaultExpandThinking(($event.target as HTMLInputElement).checked)" />
                    <span class="toggle-slider"></span>
                  </label>
                  <div class="setting-desc">{{ t('settings.general.defaultExpandThinkingDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.agentPanelPosition') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentPanelPosition === 'right' }"
                      @click="setAgentPanelPosition('right')"
                    >{{ t('settings.general.positionRight') }}</button>
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentPanelPosition === 'left' }"
                      @click="setAgentPanelPosition('left')"
                    >{{ t('settings.general.positionLeft') }}</button>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.agentPanelPositionDesc') }}</div>
                </div>
              </div>

              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M21,16.5C21,16.88 20.79,17.21 20.47,17.38L12.57,21.82C12.41,21.94 12.21,22 12,22C11.79,22 11.59,21.94 11.43,21.82L3.53,17.38C3.21,17.21 3,16.88 3,16.5V7.5C3,7.12 3.21,6.79 3.53,6.62L11.43,2.18C11.59,2.06 11.79,2 12,2C12.21,2 12.41,2.06 12.57,2.18L20.47,6.62C20.79,6.79 21,7.12 21,7.5V16.5Z"/></svg>
                  <h4>{{ t('settings.general.windowStatus') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.layoutManagement') }}</label>
                  <button class="window-reset-btn" :disabled="actionLoading" @click="resetDefaultWindows">
                    {{ t('settings.general.restoreLayout') }}
                  </button>
                </div>
                <div class="setting-desc">{{ t('settings.general.restoreLayoutDesc') }}</div>
              </div>
            </div>

            <!-- 预设编辑页 -->
            <div v-else-if="activeTab === 'presets' && editingProfile" class="tab-content">
              <!-- 基本信息卡片 -->
              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,4A4,4 0 0,1 16,8A4,4 0 0,1 12,12A4,4 0 0,1 8,8A4,4 0 0,1 12,4M12,14C16.42,14 20,15.79 20,18V20H4V18C4,15.79 7.58,14 12,14Z"/></svg>
                  <h4>{{ t('settings.profileEditor.identity') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.name') }}</label>
                  <input type="text" v-model="editingProfile.name" :placeholder="t('settings.profileEditor.namePlaceholder')" />
                </div>
              </div>

              <!-- 连接配置卡片 -->
              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2L4.5,20.29L5.21,21L12,18L18.79,21L19.5,20.29L12,2Z"/></svg>
                  <h4>{{ t('settings.profileEditor.apiConnection') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.apiFormat') }}</label>
                  <select v-model="editingProfile.config.apiFormat" class="format-select">
                    <option value="anthropic">{{ t('settings.profileEditor.anthropicNative') }}</option>
                    <option value="openai">{{ t('settings.profileEditor.openaiCompatible') }}</option>
                  </select>
                </div>
                <div class="setting-item">
                  <label>Base URL</label>
                  <input type="text" v-model="editingProfile.config.baseUrl" placeholder="https://..." />
                </div>
                <div class="setting-item">
                  <label>API Key</label>
                  <input type="password" v-model="editingProfile.config.apiKey" placeholder="sk-..." />
                </div>
              </div>

              <!-- 模型配置卡片 -->
              <div class="setting-card">
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2A10,10 0 0,1 22,12A10,10 0 0,1 12,22A10,10 0 0,1 2,12A10,10 0 0,1 12,2M12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20A8,8 0 0,0 20,12A8,8 0 0,0 12,4M12,6A6,6 0 0,1 18,12A6,6 0 0,1 12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6M12,8A4,4 0 0,0 8,12A4,4 0 0,0 12,16A4,4 0 0,0 16,12A4,4 0 0,0 12,8Z"/></svg>
                  <h4>{{ t('settings.profileEditor.modelSelection') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.mainModel') }}</label>
                  <input
                    type="text"
                    v-model="editingProfile.config.mainModel"
                    placeholder="claude-3-5-sonnet-..."
                    @input="onMainModelInput"
                  />
                  <!-- 能力徽章 -->
                  <div class="capability-badges" v-if="mainModelCaps !== null">
                    <span class="badge" :class="mainModelCaps ? 'badge-ok' : 'badge-none'">
                      <span>{{ mainModelCaps ? t('settings.profileEditor.recognized') : t('settings.profileEditor.unknownModel') }}</span>
                    </span>
                    <template v-if="mainModelCaps">
                      <span v-if="mainModelCaps.thinking" class="badge badge-think">{{ t('settings.profileEditor.thinking') }}</span>
                      <span v-if="mainModelCaps.vision" class="badge badge-ok">{{ t('settings.profileEditor.vision') }}</span>
                      <span class="badge badge-info">{{ t('settings.profileEditor.outputTokens', { count: mainModelCaps.maxTokens.toLocaleString() }) }}</span>
                    </template>
                  </div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.utilityModel') }}</label>
                  <input type="text" v-model="editingProfile.config.utilityModel" placeholder="claude-3-5-haiku-..." />
                  <div class="setting-desc">{{ t('settings.profileEditor.utilityModelDesc') }}</div>
                </div>

                <div class="setting-item">
                  <div class="setting-desc image-info">{{ t('settings.profileEditor.imageCompressInfo') }}</div>
                </div>

                <div class="advanced-toggle" @click="showAdvanced = !showAdvanced">
                  <span>{{ t('settings.profileEditor.advancedParams') }}</span>
                  <svg :class="{ rotated: showAdvanced }" viewBox="0 0 24 24" width="16" height="16"><path fill="currentColor" d="M7.41,8.58L12,13.17L16.59,8.58L18,10L12,16L6,10L7.41,8.58Z"/></svg>
                </div>

                <div v-show="showAdvanced" class="advanced-content">
                  <div class="setting-item">
                    <label>温度 (Temp)</label>
                    <input type="number" v-model.number="editingProfile.config.temperature" step="0.1" min="0" max="2" />
                  </div>
                  <div class="setting-item">
                    <label>Top-P</label>
                    <input type="number" v-model.number="editingProfile.config.topP" step="0.1" min="0" max="1" />
                  </div>
                  <div class="setting-item" v-if="editingProfile.config.apiFormat === 'anthropic'">
                    <label>Top-K</label>
                    <input type="number" v-model.number="editingProfile.config.topK" step="1" min="0" />
                  </div>
                </div>
              </div>

            </div>

            <div v-else class="empty-state">
              <svg viewBox="0 0 24 24" width="48" height="48"><path fill="currentColor" d="M12,2A10,10 0 0,1 22,12A10,10 0 0,1 12,22A10,10 0 0,1 2,12A10,10 0 0,1 12,2M12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20A8,8 0 0,0 20,12A8,8 0 0,0 12,4M12,6A6,6 0 0,1 18,12A6,6 0 0,1 12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6M12,8A4,4 0 0,0 8,12A4,4 0 0,0 12,16A4,4 0 0,0 16,12A4,4 0 0,0 12,8Z"/></svg>
              <p>{{ t('settings.profiles.empty') }}</p>
            </div>
          </div>
        </div>
      </div>

      <div class="settings-footer">
        <span class="status-msg" :class="{ 'error': isError, 'success': isSuccess }">{{ statusMsg }}</span>
        <div class="footer-actions">
          <button v-if="activeTab === 'presets'" class="save-btn" @click="save" :disabled="isSaving || actionLoading">
            {{ isSaving ? t('settings.actions.saving') : t('settings.actions.save') }}
          </button>
        </div>
      </div>
    </div>

    <ConfirmModal
      :open="!!deleteConfirm"
      :title="deleteConfirm?.title || ''"
      :message="deleteConfirm?.message || ''"
      :warning="deleteConfirm?.warning || ''"
      :confirm-text="t('settings.profiles.deleteConfirm')"
      :cancel-text="t('settings.actions.cancel')"
      confirm-kind="danger"
      :loading="actionLoading"
      @cancel="deleteConfirm = null"
      @confirm="confirmDeleteProfile"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, watch, computed, nextTick, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useTheme } from '../../composables/useTheme'
import { usePreferences, type AgentPanelPosition } from '../../composables/usePreferences'
import { useWindow } from '../../composables/useWindow'
import ConfirmModal from '../common/ConfirmModal.vue'

const { t, locale } = useI18n()

const { isDark, toggleTheme } = useTheme()
const uiPrefs = usePreferences()
const { resetWindowStates, notifyMonitorLocaleChanged } = useWindow()
const agentAudience = uiPrefs.agentAudience
const setAgentAudience = (val: "user" | "developer") => uiPrefs.setAgentAudience(val)
const agentWorkMode = uiPrefs.agentWorkMode
const setAgentWorkMode = (val: "chat" | "edit" | "plan") => uiPrefs.setAgentWorkMode(val)
const fontSize = computed(() => uiPrefs.fontSize)
const setFontSize = (val: number) => uiPrefs.setFontSize(val)
const codeFontSize = computed(() => uiPrefs.codeFontSize)
const setCodeFontSize = (val: number) => uiPrefs.setCodeFontSize(val)
const defaultExpandThinking = computed(() => uiPrefs.defaultExpandThinking)
const setDefaultExpandThinking = (val: boolean) => uiPrefs.setDefaultExpandThinking(val)
const autoScroll = computed(() => uiPrefs.autoScroll)
const setAutoScroll = (val: boolean) => uiPrefs.setAutoScroll(val)
const agentPanelPosition = computed(() => uiPrefs.agentPanelPosition)
const setAgentPanelPosition = (val: AgentPanelPosition) => uiPrefs.setAgentPanelPosition(val)
const compactMode = computed(() => uiPrefs.compactMode)
const setCompactMode = (val: boolean) => uiPrefs.setCompactMode(val)
const agentMessageOpacity = computed(() => uiPrefs.agentMessageOpacity)
const setAgentMessageOpacity = (val: number) => uiPrefs.setAgentMessageOpacity(val)
const userMessageOpacity = computed(() => uiPrefs.userMessageOpacity)
const setUserMessageOpacity = (val: number) => uiPrefs.setUserMessageOpacity(val)
const langMenuOpen = ref(false)
const localeOptions: Record<string, string> = { 'zh-CN': '简体中文', 'en-US': 'English' }
const appLocale = uiPrefs.locale
watch(appLocale, (val) => {
  locale.value = val
}, { immediate: true })
const setAppLocale = async (value: string) => {
  uiPrefs.setLocale(value as typeof appLocale.value)
  await notifyMonitorLocaleChanged(appLocale.value)
  await nextTick()
}

const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void
}>()

// UI 状态
const activeTab = ref<'general' | 'presets'>('general')
const showAdvanced = ref(false)

interface AgentConfig {
  apiFormat: string
  apiKey: string
  baseUrl: string
  mainModel: string
  utilityModel: string
  enableThinking?: boolean
  temperature?: number | null
  topP?: number | null
  topK?: number | null
}

interface ModelCapabilities {
  streaming: boolean
  thinking: boolean
  thinkingParam?: string
  temperature: boolean
  vision: boolean
  maxTokens: number
  maxContextTokens?: number | null
  notes: string
}

interface ModelProfile {
  id: string
  name: string
  config: AgentConfig
}

interface AppConfig {
  activeProfileId: string
  globalProfileId: string
  profiles: ModelProfile[]
}

const createEmptyConfig = (): AppConfig => ({
  activeProfileId: 'default',
  globalProfileId: 'default',
  profiles: []
})

const createBlankProfile = (id: string): ModelProfile => ({
  id,
  name: t('settings.profiles.newName'),
  config: {
    apiFormat: 'openai',
    apiKey: '',
    baseUrl: '',
    mainModel: '',
    utilityModel: '',
    enableThinking: false,
    temperature: null,
    topP: null,
    topK: null,
  }
})

const cloneConfig = <T>(value: T): T => JSON.parse(JSON.stringify(value))

const normalizeProfileConfig = (config: AppConfig) => {
  config.profiles.forEach((p) => {
    p.config.temperature = p.config.temperature == null ? null : Number(p.config.temperature)
    p.config.topP = p.config.topP == null ? null : Number(p.config.topP)
    p.config.topK = p.config.topK == null ? null : Number(p.config.topK)
  })
  return config
}

const ensureValidSelection = (config: AppConfig, preferredId?: string) => {
  if (config.profiles.length === 0) {
    config.activeProfileId = 'default'
    config.globalProfileId = 'default'
    selectedProfileId.value = 'default'
    return
  }

  const fallbackId = preferredId && config.profiles.find((p) => p.id === preferredId)
    ? preferredId
    : config.profiles[0].id

  if (!config.profiles.find((p) => p.id === config.activeProfileId)) {
    config.activeProfileId = config.profiles[0].id
  }
  if (!config.profiles.find((p) => p.id === config.globalProfileId)) {
    config.globalProfileId = config.profiles[0].id
  }
  if (!config.profiles.find((p) => p.id === selectedProfileId.value)) {
    selectedProfileId.value = fallbackId
  }
}

const syncConfigs = (config: AppConfig, preferredId?: string) => {
  const normalized = normalizeProfileConfig(cloneConfig(config))
  ensureValidSelection(normalized, preferredId)
  savedConfig.value = cloneConfig(normalized)
  draftConfig.value = cloneConfig(normalized)
}

const savedConfig = ref<AppConfig>(createEmptyConfig())
const draftConfig = ref<AppConfig>(createEmptyConfig())
const selectedProfileId = ref('default')
const isSaving = ref(false)
const actionLoading = ref(false)
const statusMsg = ref('')
const isError = ref(false)
const isSuccess = ref(false)
const deleteConfirm = ref<{ id: string; title: string; message: string; warning: string } | null>(null)

const editingProfile = computed(() => {
  return draftConfig.value.profiles.find(p => p.id === selectedProfileId.value)
})

const mainModelCaps = ref<ModelCapabilities | null | undefined>(undefined)
let capQueryTimer: ReturnType<typeof setTimeout> | null = null

const resetStatus = () => {
  statusMsg.value = ''
  isError.value = false
  isSuccess.value = false
}

const setErrorStatus = (message: string) => {
  statusMsg.value = message
  isError.value = true
  isSuccess.value = false
}

const setSuccessStatus = (message: string) => {
  statusMsg.value = message
  isError.value = false
  isSuccess.value = true
}

const resetDefaultWindows = async () => {
  if (actionLoading.value) return

  actionLoading.value = true
  resetStatus()
  try {
    await resetWindowStates()
    setSuccessStatus(t('settings.general.restoreLayoutSuccess'))
  } catch (e) {
    console.error('恢复默认窗口失败:', e)
    setErrorStatus(t('settings.general.restoreLayoutError', { error: String(e) }))
  } finally {
    actionLoading.value = false
  }
}

const onMainModelInput = () => {
  if (capQueryTimer) clearTimeout(capQueryTimer)
  mainModelCaps.value = undefined
  capQueryTimer = setTimeout(async () => {
    const modelId = editingProfile.value?.config.mainModel?.trim()
    if (!modelId) {
      mainModelCaps.value = undefined
      return
    }
    try {
      const caps = await invoke<ModelCapabilities | null>('get_model_capabilities', { modelId })
      mainModelCaps.value = caps
    } catch {
      mainModelCaps.value = null
    }
  }, 400)
}

watch(selectedProfileId, () => {
  const modelId = editingProfile.value?.config.mainModel?.trim()
  if (modelId) onMainModelInput()
  else mainModelCaps.value = undefined
})

const loadConfig = async () => {
  try {
    const res = await invoke<AppConfig>('get_config')
    syncConfigs(res, res.activeProfileId || res.profiles[0]?.id)
  } catch (e) {
    console.error('Failed to load config:', e)
  }
}

watch(() => props.modelValue, (newVal) => {
  if (newVal) {
    resetStatus()
    deleteConfirm.value = null
    loadConfig()
  }
})

const selectProfile = (id: string) => {
  selectedProfileId.value = id
  activeTab.value = 'presets'
}

const addProfile = () => {
  resetStatus()
  const newId = `profile_${Date.now()}`
  draftConfig.value.profiles.push(createBlankProfile(newId))
  selectedProfileId.value = newId
  activeTab.value = 'presets'
}

const copyProfile = (id: string) => {
  resetStatus()
  const source = draftConfig.value.profiles.find(p => p.id === id)
  if (!source) return
  const newId = `profile_${Date.now()}`
  const copy = JSON.parse(JSON.stringify(source)) as typeof source
  copy.id = newId
  copy.name = `${copy.name} - ${t('settings.profiles.copySuffix')}`
  draftConfig.value.profiles.push(copy)
  selectedProfileId.value = newId
  activeTab.value = 'presets'
}

// ── 拖拽排序 ──
const dragIndex = ref<number | null>(null)
const dragOverIndex = ref<number | null>(null)
const dragOffsetY = ref(0)
let dragStartY = 0
let dragStartIndex = 0
let dragItemHeight = 0
let dragMoved = false

const DRAG_THRESHOLD = 5

const onMouseDown = (e: MouseEvent, index: number) => {
  if (actionLoading.value) return
  dragStartY = e.clientY
  dragStartIndex = index
  dragItemHeight = (e.currentTarget as HTMLElement).offsetHeight
  dragMoved = false
  dragIndex.value = index
}

const onMouseMove = (e: MouseEvent) => {
  if (dragIndex.value === null) return
  const deltaY = e.clientY - dragStartY
  if (!dragMoved && Math.abs(deltaY) > DRAG_THRESHOLD) {
    dragMoved = true
    document.body.style.userSelect = 'none'
  }
  if (!dragMoved) return
  dragOffsetY.value = deltaY
  // 计算当前悬停位置（鼠标所在处对应哪个预设）
  const profiles = draftConfig.value.profiles
  const relativeIdx = Math.round(deltaY / dragItemHeight)
  const targetIdx = Math.max(0, Math.min(profiles.length - 1, dragStartIndex + relativeIdx))
  if (targetIdx !== dragStartIndex || dragOverIndex.value === null) {
    dragOverIndex.value = targetIdx
  }
}

const onMouseUp = (_e: MouseEvent, _index: number) => {
  if (dragIndex.value === null) return
  document.body.style.userSelect = ''
  if (!dragMoved) {
    selectProfile(draftConfig.value.profiles[dragStartIndex].id)
  } else if (dragOverIndex.value !== null && dragOverIndex.value !== dragStartIndex) {
    const profiles = draftConfig.value.profiles
    const [moved] = profiles.splice(dragStartIndex, 1)
    profiles.splice(dragOverIndex.value, 0, moved)
  }
  dragIndex.value = null
  dragOverIndex.value = null
  dragOffsetY.value = 0
  dragMoved = false
}

const onWindowMouseUp = (e: MouseEvent) => {
  if (dragIndex.value !== null) {
    document.body.style.userSelect = ''
    dragIndex.value = null
    dragOverIndex.value = null
    dragOffsetY.value = 0
    dragMoved = false
  }
  if (langMenuOpen.value && e.target instanceof HTMLElement && !e.target.closest('.custom-select')) {
    langMenuOpen.value = false
  }
}

onMounted(() => window.addEventListener('mouseup', onWindowMouseUp))
onUnmounted(() => window.removeEventListener('mouseup', onWindowMouseUp))

const toggleGlobalProfile = async (profileId: string) => {
  if (actionLoading.value || savedConfig.value.globalProfileId === profileId) return

  actionLoading.value = true
  resetStatus()
  try {
    const nextConfig = cloneConfig(savedConfig.value)
    nextConfig.globalProfileId = profileId
    ensureValidSelection(nextConfig, selectedProfileId.value)
    await invoke('save_config_cmd', { newConfig: nextConfig })
    syncConfigs(nextConfig, selectedProfileId.value)
    setSuccessStatus(t('settings.profiles.globalUpdated'))
  } catch (e) {
    console.error('保存全局预设失败:', e)
    setErrorStatus(t('settings.status.globalSaveError', { error: String(e) }))
  } finally {
    actionLoading.value = false
  }
}

const requestDeleteProfile = (id: string) => {
  if (draftConfig.value.profiles.length <= 1) return

  const profile = draftConfig.value.profiles.find(p => p.id === id)
  if (!profile) return

  deleteConfirm.value = {
    id,
    title: t('settings.profiles.deleteTitle'),
    message: t('settings.profiles.deleteMessage', { name: profile.name }),
    warning: t('settings.profiles.deleteWarning')
  }
}

const confirmDeleteProfile = async () => {
  if (!deleteConfirm.value || actionLoading.value) return

  actionLoading.value = true
  resetStatus()
  try {
    const targetId = deleteConfirm.value.id
    const nextConfig = cloneConfig(savedConfig.value)
    const index = nextConfig.profiles.findIndex((p) => p.id === targetId)
    if (index === -1) {
      deleteConfirm.value = null
      return
    }

    nextConfig.profiles.splice(index, 1)
    if (nextConfig.profiles.length === 0) {
      throw new Error(t('settings.validation.keepOneProfile'))
    }

    const fallbackId = nextConfig.profiles[0].id
    if (nextConfig.activeProfileId === targetId) {
      nextConfig.activeProfileId = fallbackId
    }
    if (nextConfig.globalProfileId === targetId) {
      nextConfig.globalProfileId = fallbackId
    }

    const nextSelectedId = selectedProfileId.value === targetId ? fallbackId : selectedProfileId.value
    ensureValidSelection(nextConfig, nextSelectedId)
    await invoke('save_config_cmd', { newConfig: nextConfig })
    syncConfigs(nextConfig, nextSelectedId)
    deleteConfirm.value = null
    setSuccessStatus(t('settings.profiles.deleteSuccess'))
  } catch (e) {
    console.error('删除配置预设失败:', e)
    setErrorStatus(t('settings.status.deleteError', { error: String(e) }))
  } finally {
    actionLoading.value = false
  }
}

const hasMeaningfulDraftValue = (value: unknown): boolean => {
  if (value === null || value === undefined || value === false) return false
  if (typeof value === 'number') return Number.isFinite(value)
  return String(value).trim().length > 0
}

const hasNewProfileContent = (profile: ModelProfile): boolean => {
  const blank = createBlankProfile(profile.id)
  if (profile.name.trim() && profile.name.trim() !== blank.name) return true
  if (profile.config.apiFormat !== blank.config.apiFormat) return true
  if ((profile.config.enableThinking ?? false) !== (blank.config.enableThinking ?? false)) return true

  const contentKeys: Array<keyof AgentConfig> = [
    'apiKey',
    'baseUrl',
    'mainModel',
    'utilityModel',
    'temperature',
    'topP',
    'topK',
  ]

  return contentKeys.some((key) => hasMeaningfulDraftValue(profile.config[key]))
}

const persistFilledNewProfilesBeforeClose = async () => {
  const savedIds = new Set(savedConfig.value.profiles.map((profile) => profile.id))
  const filledNewProfiles = draftConfig.value.profiles.filter((profile) => {
    return !savedIds.has(profile.id) && hasNewProfileContent(profile)
  })

  if (filledNewProfiles.length === 0) return

  const nextConfig = cloneConfig(savedConfig.value)
  nextConfig.profiles.push(...filledNewProfiles.map((profile) => cloneConfig(profile)))
  ensureValidSelection(nextConfig, selectedProfileId.value)
  const normalized = normalizeProfileConfig(nextConfig)

  await invoke('save_config_cmd', { newConfig: normalized })
  syncConfigs(normalized, selectedProfileId.value)
}

const close = async () => {
  if (isSaving.value || actionLoading.value) return

  try {
    await persistFilledNewProfilesBeforeClose()
  } catch (e) {
    console.error('保存已填写的新预设失败:', e)
    setErrorStatus(t('settings.status.filledProfileSaveError', { error: String(e) }))
    return
  }

  draftConfig.value = cloneConfig(savedConfig.value)
  ensureValidSelection(draftConfig.value, selectedProfileId.value)
  deleteConfirm.value = null
  resetStatus()
  emit('update:modelValue', false)
}

const save = async () => {
  if (!draftConfig.value.profiles || draftConfig.value.profiles.length === 0) return

  ensureValidSelection(draftConfig.value, selectedProfileId.value)

  for (const p of draftConfig.value.profiles) {
    if (!p.name || !p.name.trim()) {
      setErrorStatus(t('settings.validation.nameRequired'))
      return
    }
    if (!p.config.baseUrl || !p.config.baseUrl.trim()) {
      setErrorStatus(t('settings.validation.baseUrlRequired', { name: p.name }))
      return
    }
    if (!p.config.mainModel || !p.config.mainModel.trim()) {
      setErrorStatus(t('settings.validation.mainModelRequired', { name: p.name }))
      return
    }
    if (!p.config.utilityModel || !p.config.utilityModel.trim()) {
      setErrorStatus(t('settings.validation.utilityModelRequired', { name: p.name }))
      return
    }
  }

  isSaving.value = true
  resetStatus()

  try {
    const nextConfig = normalizeProfileConfig(cloneConfig(draftConfig.value))
    ensureValidSelection(nextConfig, selectedProfileId.value)
    await invoke('save_config_cmd', { newConfig: nextConfig })
    syncConfigs(nextConfig, selectedProfileId.value)
    setSuccessStatus(t('settings.status.saveSuccess'))
    setTimeout(() => {
      close()
    }, 800)
  } catch (e) {
    setErrorStatus(t('settings.status.saveError', { error: String(e) }))
  } finally {
    isSaving.value = false
  }
}
</script>

<style scoped>
.settings-overlay {
  position: fixed;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  justify-content: center;
  align-items: center;
  z-index: 1000;
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  animation: fadeIn var(--transition-fast);
}

@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

.settings-modal {
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  width: min(1080px, calc(100vw - 48px));
  max-width: 96vw;
  height: min(800px, calc(100vh - 48px));
  max-height: 92vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 24px 70px rgba(15, 23, 42, 0.22), var(--glass-shadow);
  animation: slideIn var(--transition-normal);
  overflow: hidden;
}

@keyframes slideIn {
  from { opacity: 0; transform: scale(0.96) translateY(12px); }
  to { opacity: 1; transform: scale(1) translateY(0); }
}

.settings-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  min-height: 58px;
  padding: 14px 22px 14px 24px;
  border-bottom: 1px solid var(--glass-border);
  background: var(--surface-strong);
}

.header-title {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--text-main);
}

.header-icon {
  color: var(--accent-blue);
}

.settings-header h3 {
  margin: 0;
  font-size: 1.1rem;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.settings-container {
  flex: 1;
  display: flex;
  overflow: hidden;
}

/* 侧边栏样式 */
.settings-sidebar {
  width: 240px;
  flex: 0 0 240px;
  border-right: 1px solid var(--glass-border);
  background: color-mix(in srgb, var(--surface-strong) 92%, var(--glass-bg-light));
  display: flex;
  flex-direction: column;
}

.sidebar-nav {
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  border-radius: var(--radius-md);
  color: var(--text-muted);
  font-size: 14px;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.nav-item:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.nav-item.active {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-blue);
}

.sidebar-divider {
  height: 1px;
  background: var(--glass-border);
  margin: 8px 12px;
}

.sidebar-section-header {
  padding: 12px 18px 8px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  color: var(--text-muted);
  font-size: 0.75rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.add-btn {
  background: var(--glass-bg-light);
  color: var(--accent-blue);
  border: 1px solid rgba(59, 130, 246, 0.3);
  width: 20px;
  height: 20px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  font-size: 14px;
  transition: all var(--transition-fast);
}

.add-btn:hover {
  background: rgba(59, 130, 246, 0.1);
  border-color: var(--accent-blue);
}

.profile-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px 12px 12px;
}

.profile-item {
  padding: 8px 10px 8px 12px;
  border-radius: var(--radius-md);
  cursor: pointer;
  margin-bottom: 2px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  transition: all var(--transition-fast);
  color: var(--text-muted);
  font-size: 13px;
}

.profile-item:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.profile-item.active {
  background: rgba(59, 130, 246, 0.08);
  color: var(--accent-blue);
  font-weight: 500;
}

.profile-name {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
}

.profile-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.sidebar-switch {
  position: relative;
  display: inline-block;
  width: 24px;
  height: 14px;
}

.sidebar-switch input { opacity: 0; width: 0; height: 0; }
.sidebar-switch .slider {
  position: absolute;
  cursor: pointer;
  top: 0; left: 0; right: 0; bottom: 0;
  background-color: var(--border-color);
  transition: .3s;
  border-radius: 14px;
}

.sidebar-switch .slider:before {
  position: absolute;
  content: "";
  height: 10px; width: 10px;
  left: 2px; bottom: 2px;
  background-color: white;
  transition: .3s;
  border-radius: 50%;
}

.sidebar-switch input:checked + .slider { background-color: var(--accent-green); }
.sidebar-switch input:checked + .slider:before { transform: translateX(10px); }

.copy-btn, .delete-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  opacity: 0;
  cursor: pointer;
  padding: 2px;
}

.profile-item:hover .copy-btn, .profile-item:hover .delete-btn { opacity: 1; }
.copy-btn:hover { color: var(--accent-blue); }
.delete-btn:hover { color: var(--accent-red); }

.profile-item { cursor: grab; }
.profile-item.dragging { opacity: 0.5; cursor: grabbing; }
.profile-item.drag-over {
  border-top: 2px solid var(--accent-blue);
}

/* 内容区域样式 */
.settings-content {
  flex: 1;
  min-width: 0;
  background: color-mix(in srgb, var(--surface-strong) 96%, var(--glass-bg-heavy));
}

.settings-body {
  height: 100%;
  padding: 24px 32px;
  overflow-y: auto;
}

.tab-content {
  display: flex;
  flex-direction: column;
  gap: 20px;
  max-width: 800px;
  margin: 0 auto;
}

.setting-card {
  background: var(--surface-strong);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  padding: 20px;
  box-shadow: 0 2px 8px rgba(0,0,0,0.04);
}

.card-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 20px;
  color: var(--text-main);
  border-bottom: 1px solid var(--glass-border-subtle);
  padding-bottom: 12px;
}

.card-header h4 {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
}

.setting-item {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-bottom: 18px;
}

.setting-item:last-child { margin-bottom: 0; }

.setting-item label {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-main);
}

.setting-item input, .setting-item select {
  width: 100%;
  height: 38px;
  padding: 0 12px;
  background-color: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--text-main);
  font-size: 13px;
  transition: border-color 0.2s, box-shadow 0.2s, background-color 0.2s;
}

.setting-item select {
  appearance: none;
  -webkit-appearance: none;
  background-image: url("data:image/svg+xml;charset=US-ASCII,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2214%22%20height%3D%2214%22%20viewBox%3D%220%200%2024%2024%22%20fill%3D%22none%22%20stroke%3D%22%2364748b%22%20stroke-width%3D%222%22%20stroke-linecap%3D%22round%22%20stroke-linejoin%3D%22round%22%3E%3Cpolyline%20points%3D%226%209%2012%2015%2018%209%22%3E%3C%2Fpolyline%3E%3C%2Fsvg%3E");
  background-repeat: no-repeat;
  background-position: right 12px center;
  background-size: 14px;
  padding-right: 32px;
  cursor: pointer;
}

.setting-item select:hover {
  border-color: var(--accent-blue);
  background-color: var(--glass-bg);
}

.format-select {
  width: auto !important;
  min-width: 160px;
}

/* ── 自定义下拉（替代原生 select）── */
.custom-select {
  position: relative;
  width: auto;
  min-width: 160px;
}

.custom-select-trigger {
  width: 100%;
  height: 38px;
  padding: 0 12px;
  padding-right: 32px;
  background-color: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--text-main);
  font-size: 13px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  cursor: pointer;
  transition: border-color 0.2s, box-shadow 0.2s, background-color 0.2s;
}

.custom-select-trigger:hover {
  border-color: var(--accent-blue);
  background-color: var(--glass-bg);
}

.custom-select.open .custom-select-trigger {
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.1);
}

.custom-select-trigger svg {
  flex-shrink: 0;
  color: var(--text-muted);
  transition: transform 0.2s;
}

.custom-select.open .custom-select-trigger svg {
  transform: rotate(180deg);
}

.custom-select-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  right: 0;
  z-index: 110;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 20%, transparent);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(15, 23, 42, 0.12);
  overflow: hidden;
  padding: 4px;
  animation: popIn var(--transition-fast);
}

.custom-select-option {
  padding: 8px 12px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 13px;
  color: var(--text-main);
  transition: background-color 0.15s;
}

.custom-select-option:hover {
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}

.custom-select-option.active {
  background: color-mix(in srgb, var(--accent-blue) 16%, transparent);
  color: var(--accent-blue);
  font-weight: 600;
}

:global(body.dark-mode) .setting-item select {
  background-image: url("data:image/svg+xml;charset=US-ASCII,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2214%22%20height%3D%2214%22%20viewBox%3D%220%200%2024%2024%22%20fill%3D%22none%22%20stroke%3D%22%23a1a1aa%22%20stroke-width%3D%222%22%20stroke-linecap%3D%22round%22%20stroke-linejoin%3D%22round%22%3E%3Cpolyline%20points%3D%226%209%2012%2015%2018%209%22%3E%3C%2Fpolyline%3E%3C%2Fsvg%3E");
}

.setting-item select option {
  background: var(--surface-strong);
  color: var(--text-main);
}

.setting-item input:focus, .setting-item select:focus {
  outline: none;
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.1);
}

.setting-desc {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.5;
}

/* 字体大小控件 */
.font-size-control {
  display: flex;
  align-items: center;
  gap: 10px;
}

.font-size-step {
  width: 34px;
  height: 34px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  background: var(--glass-bg);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  color: var(--text-muted);
  cursor: pointer;
  transition: all 0.2s ease;
  flex-shrink: 0;
}

.font-size-step:hover:not(:disabled) {
  background: var(--glass-bg-heavy);
  border-color: var(--accent-blue);
  color: var(--accent-blue);
  transform: scale(1.04);
  box-shadow: 0 0 12px rgba(59, 130, 246, 0.15);
}

.font-size-step:active:not(:disabled) {
  transform: scale(0.96);
}

.font-size-step:disabled {
  opacity: 0.28;
  cursor: not-allowed;
}

.slider-track-wrap {
  flex: 1;
  display: flex;
  align-items: center;
}

.font-size-slider {
  width: 100%;
  height: 22px;
  -webkit-appearance: none;
  appearance: none;
  background: transparent;
  outline: none;
  cursor: pointer;
  margin: 0;
}

/* 轨道 */
.font-size-slider::-webkit-slider-runnable-track {
  height: 5px;
  border-radius: 3px;
  background: linear-gradient(
    to right,
    var(--accent-blue) 0%,
    var(--accent-blue) var(--fill-pct, 50%),
    var(--glass-bg-light) var(--fill-pct, 50%),
    var(--glass-bg-light) 100%
  );
  border: 0.5px solid var(--glass-border-subtle);
}

/* 滑块钮 */
.font-size-slider::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--surface-strong);
  border: 2px solid var(--accent-blue);
  box-shadow: 0 2px 8px rgba(59, 130, 246, 0.25), 0 0 0 4px rgba(59, 130, 246, 0.08);
  margin-top: -7px;
  cursor: pointer;
  transition: box-shadow 0.2s ease, transform 0.15s ease;
}

.font-size-slider::-webkit-slider-thumb:hover {
  box-shadow: 0 2px 12px rgba(59, 130, 246, 0.4), 0 0 0 6px rgba(59, 130, 246, 0.12);
}

.font-size-slider::-webkit-slider-thumb:active {
  transform: scale(0.92);
}

/* Firefox */
.font-size-slider::-moz-range-track {
  height: 5px;
  border-radius: 3px;
  background: var(--glass-bg-light);
  border: 0.5px solid var(--glass-border-subtle);
}

.font-size-slider::-moz-range-progress {
  height: 5px;
  border-radius: 3px;
  background: var(--accent-blue);
}

.font-size-slider::-moz-range-thumb {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--surface-strong);
  border: 2px solid var(--accent-blue);
  box-shadow: 0 2px 8px rgba(59, 130, 246, 0.25);
  cursor: pointer;
}

.font-size-value {
  font-size: 12px;
  font-weight: 650;
  color: var(--text-main);
  background: var(--glass-bg);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  border: 1px solid var(--glass-border-subtle);
  border-radius: 6px;
  padding: 4px 10px;
  min-width: 44px;
  text-align: center;
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

/* 开关切换 */
.toggle-switch {
  position: relative;
  display: inline-block;
  width: 44px;
  height: 24px;
  cursor: pointer;
}

.toggle-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}

.toggle-slider {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  transition: all 0.2s;
}

.toggle-slider::before {
  content: "";
  position: absolute;
  height: 18px;
  width: 18px;
  left: 2px;
  bottom: 2px;
  background: var(--text-muted);
  border-radius: 50%;
  transition: all 0.2s;
}

.toggle-switch input:checked + .toggle-slider {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
}

.toggle-switch input:checked + .toggle-slider::before {
  transform: translateX(20px);
  background: white;
}

/* 按钮组样式 */
.display-mode-toggle {
  display: flex;
  background: var(--glass-bg-light);
  padding: 4px;
  border-radius: 8px;
  width: fit-content;
}

.display-mode-btn {
  padding: 6px 16px;
  border: none;
  background: transparent;
  border-radius: 6px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-muted);
  cursor: pointer;
  transition: all 0.2s;
}

.display-mode-btn.active {
  background: var(--surface-strong);
  color: var(--accent-blue);
  box-shadow: 0 2px 4px rgba(0,0,0,0.1);
}

.theme-toggle-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  width: fit-content;
  padding: 8px 16px;
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--text-main);
  font-size: 13px;
  cursor: pointer;
}

.window-reset-btn {
  width: fit-content;
  padding: 8px 16px;
  background: transparent;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--accent-red);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}

/* 高级参数折叠 */
.advanced-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  padding: 10px 0;
  color: var(--accent-blue);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  border-top: 1px dashed var(--glass-border);
}

.advanced-toggle svg {
  transition: transform 0.2s;
}

.advanced-toggle svg.rotated {
  transform: rotate(180deg);
}

.advanced-content {
  padding-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

/* 网格布局 */
.grid-3 {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px;
}

.sub-item {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.sub-item label {
  font-size: 11px;
  color: var(--text-muted);
}

/* 能力徽章 */
.capability-badges {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 4px;
}

.badge {
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
}

.badge-ok { background: rgba(16, 185, 129, 0.1); color: var(--accent-green); }
.badge-think { background: rgba(139, 92, 246, 0.1); color: #a080f0; }
.badge-info { background: rgba(59, 130, 246, 0.1); color: var(--accent-blue); }
.badge-none { background: rgba(100, 116, 139, 0.1); color: var(--text-muted); }

.empty-state {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  gap: 16px;
}

.settings-footer {
  min-height: 64px;
  padding: 12px 24px;
  border-top: 1px solid var(--glass-border);
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: var(--surface-strong);
}

.save-btn {
  background: var(--accent-blue);
  color: white;
  border: none;
  padding: 10px 24px;
  border-radius: 8px;
  font-size: 14px;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s;
}

.save-btn:hover:not(:disabled) {
  opacity: 0.9;
  transform: translateY(-1px);
}

.save-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.status-msg {
  font-size: 13px;
  font-weight: 500;
}

.status-msg.error { color: var(--accent-red); }
.status-msg.success { color: var(--accent-green); }

.icon-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
  display: flex;
  transition: all 0.2s;
}

.icon-btn:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}
</style>

