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
            <div
              class="nav-item"
              :class="{ active: activeTab === 'prompts' }"
              @click="activeTab = 'prompts'"
            >
              <svg viewBox="0 0 24 24" width="18" height="18">
                <path fill="currentColor" d="M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20M8.5,11.5L10,10L12,12L14,10L15.5,11.5L13.5,13.5L15.5,15.5L14,17L12,15L10,17L8.5,15.5L10.5,13.5L8.5,11.5Z"/>
              </svg>
              <span>{{ t('settings.tabs.prompts') }}</span>
            </div>
            <div
              class="nav-item"
              :class="{ active: activeTab === 'tools' }"
              @click="activeTab = 'tools'"
            >
              <svg viewBox="0 0 24 24" width="18" height="18">
                <path fill="currentColor" d="M22.7,19L13.6,9.9C14.5,7.6 14,4.9 12.1,3C10.1,1 7.1,0.6 4.7,1.7L9,6L6,9L1.6,4.7C0.4,7.1 0.9,10.1 2.9,12.1C4.8,14 7.5,14.5 9.8,13.6L18.9,22.7C19.3,23.1 19.9,23.1 20.3,22.7L22.6,20.4C23.1,20 23.1,19.3 22.7,19Z" />
              </svg>
              <span>{{ t('settings.tabs.tools') }}</span>
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
          <div class="settings-body" :class="{ 'no-scroll': activeTab === 'prompts' }">
            <!-- 常规设置页 -->
            <div v-if="activeTab === 'general'" class="tab-content">
              <!--
                分区原则：卡片顺序 = 使用频率（外观最常动 → 窗口低频兜底）；
                每张卡回答用户找设置时的一个心理提问：
                「界面长什么样 / Agent 怎么干活 / 消息怎么处理 / 数据安不安全 / 窗口怎么摆」。
              -->
              <div class="setting-card">
                <!-- 外观：颜色、语言、字体、密度、消息气泡观感 -->
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
                  <label>{{ t('settings.general.fontSize') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      v-for="p in fontSizePresets"
                      :key="p.value"
                      class="display-mode-btn"
                      :class="{ active: fontSize === p.value }"
                      @click="setFontSize(p.value)"
                    >{{ t(p.label) }}</button>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.fontSizeDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.codeFontSize') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      v-for="p in codeFontSizePresets"
                      :key="p.value"
                      class="display-mode-btn"
                      :class="{ active: codeFontSize === p.value }"
                      @click="setCodeFontSize(p.value)"
                    >{{ t(p.label) }}</button>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.codeFontSizeDesc') }}</div>
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
                <!-- Agent 交互：视图模式、工作模式、审批、思考、反思——Agent 怎么干活、怎么呈现过程 -->
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M12,4A8,8 0 0,1 20,12A8,8 0 0,1 12,20A8,8 0 0,1 4,12A8,8 0 0,1 12,4M12,17A5,5 0 0,1 7,12A5,5 0 0,1 12,7A5,5 0 0,1 17,12A5,5 0 0,1 12,17Z"/></svg>
                  <h4>{{ t('settings.general.agentInteraction') }}</h4>
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
                  <label>{{ t('settings.general.approvalMode') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentApprovalMode === 'request_approval' }"
                      @click="setAgentApprovalMode('request_approval')"
                    >{{ t('settings.general.request_approval') }}</button>
                    <button
                      class="display-mode-btn"
                      :class="{ active: agentApprovalMode === 'auto_approve' }"
                      @click="setAgentApprovalMode('auto_approve')"
                    >{{ t('settings.general.auto_approve') }}</button>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.approvalModeDesc') }}</div>
                </div>
                <!--
                  深度思考默认档位：与工作模式/权限档位并列的**设置默认值**（UiPreferences）。
                  注意它只管「新建会话首次发消息时固化的值」，改它**不会**回溯影响已有会话
                  （正因如此它必须与预设解耦——挂在预设上会导致改一个预设倒灌所有会话）。
                -->
                <div class="setting-item">
                  <label>{{ t('settings.general.thinkingDefault') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      class="display-mode-btn"
                      :class="{ active: thinkingDefault === 'follow_global' }"
                      @click="setThinkingDefault('follow_global')"
                    >{{ t('settings.general.thinkingFollowGlobal') }}</button>
                    <button
                      class="display-mode-btn"
                      :class="{ active: thinkingDefault === 'on' }"
                      @click="setThinkingDefault('on')"
                    >{{ t('settings.general.thinkingOn') }}</button>
                    <button
                      class="display-mode-btn"
                      :class="{ active: thinkingDefault === 'off' }"
                      @click="setThinkingDefault('off')"
                    >{{ t('settings.general.thinkingOff') }}</button>
                  </div>
                  <div class="setting-desc">{{ t('settings.general.thinkingDefaultDesc') }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.reflectionMode') }}</label>
                  <div class="custom-select" :class="{ open: reflectionMenuOpen }">
                    <button
                      class="custom-select-trigger"
                      :title="t('settings.general.reflectionModeTooltip')"
                      @click="reflectionMenuOpen = !reflectionMenuOpen"
                    >
                      <span>{{ t(reflectionMode === 'always' ? 'settings.general.reflectionAlways' : reflectionMode === 'off' ? 'settings.general.reflectionOff' : 'settings.general.reflectionSmart') }}</span>
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
                    </button>
                    <div v-if="reflectionMenuOpen" class="custom-select-menu">
                      <div
                        class="custom-select-option"
                        :class="{ active: reflectionMode === 'smart' }"
                        @click="setReflectionMode('smart'); reflectionMenuOpen = false"
                      >{{ t('settings.general.reflectionSmart') }}</div>
                      <div
                        class="custom-select-option"
                        :class="{ active: reflectionMode === 'always' }"
                        @click="setReflectionMode('always'); reflectionMenuOpen = false"
                      >{{ t('settings.general.reflectionAlways') }}</div>
                      <div
                        class="custom-select-option"
                        :class="{ active: reflectionMode === 'off' }"
                        @click="setReflectionMode('off'); reflectionMenuOpen = false"
                      >{{ t('settings.general.reflectionOff') }}</div>
                    </div>
                  </div>
                  <div class="setting-desc">{{ t(`settings.general.reflection${reflectionMode === 'always' ? 'Always' : reflectionMode === 'off' ? 'Off' : 'Smart'}Desc`) }}</div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.defaultExpandThinking') }}</label>
                  <label class="toggle-switch">
                    <input type="checkbox" :checked="defaultExpandThinking" @change="setDefaultExpandThinking(($event.target as HTMLInputElement).checked)" />
                    <span class="toggle-slider"></span>
                  </label>
                  <div class="setting-desc">{{ t('settings.general.defaultExpandThinkingDesc') }}</div>
                </div>
              </div>

              <div class="setting-card">
                <!-- 对话与消息：消息流的表现方式与发消息时的处理 -->
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M20,2H4A2,2 0 0,0 2,4V22L6,18H20A2,2 0 0,0 22,16V4A2,2 0 0,0 20,2Z"/></svg>
                  <h4>{{ t('settings.general.conversation') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.autoScroll') }}</label>
                  <label class="toggle-switch">
                    <input type="checkbox" :checked="autoScroll" @change="setAutoScroll(($event.target as HTMLInputElement).checked)" />
                    <span class="toggle-slider"></span>
                  </label>
                  <div class="setting-desc">{{ t('settings.general.autoScrollDesc') }}</div>
                </div>
                <!--
                  图片压缩档位：全局偏好（UiPreferences），**所有预设共用**、不随预设切换而变。
                  因此放在常规设置里，而不是模型预设页；改档位立刻生效，无需点保存。
                -->
                <div class="setting-item">
                  <label>{{ t('settings.general.imageCompressTier') }}</label>
                  <div class="display-mode-toggle">
                    <button
                      v-for="tier in imageCompressTiers"
                      :key="tier.value"
                      class="display-mode-btn"
                      :class="{ active: imageCompressTier === tier.value }"
                      :title="t(tier.tooltip)"
                      @click="setImageCompressTier(tier.value)"
                    >{{ t(tier.label) }}</button>
                  </div>
                  <div class="setting-desc">{{ t(`settings.general.imageCompressTier${imageCompressTier === 'eco' ? 'Eco' : imageCompressTier === 'hd' ? 'Hd' : 'Standard'}Desc`) }}</div>
                </div>
              </div>

              <div class="setting-card">
                <!-- 可靠性与数据：崩溃/断电场景下的会话数据保全 -->
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12,2L4,5V11C4,16.25 7.4,21.15 12,22C16.6,21.15 20,16.25 20,11V5L12,2Z"/></svg>
                  <h4>{{ t('settings.general.reliability') }}</h4>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.general.crashProtection') }}</label>
                  <label class="toggle-switch">
                    <input type="checkbox" :checked="crashProtection" @change="setCrashProtection(($event.target as HTMLInputElement).checked)" />
                    <span class="toggle-slider"></span>
                  </label>
                  <div class="setting-desc">{{ t('settings.general.crashProtectionDesc') }}</div>
                </div>
              </div>

              <div class="setting-card">
                <!-- 窗口与布局：监控窗口摆放与布局兜底 -->
                <div class="card-header">
                  <svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M21,16.5C21,16.88 20.79,17.21 20.47,17.38L12.57,21.82C12.41,21.94 12.21,22 12,22C11.79,22 11.59,21.94 11.43,21.82L3.53,17.38C3.21,17.21 3,16.88 3,16.5V7.5C3,7.12 3.21,6.79 3.53,6.62L11.43,2.18C11.59,2.06 11.79,2 12,2C12.21,2 12.41,2.06 12.57,2.18L20.47,6.62C20.79,6.79 21,7.12 21,7.5V16.5Z"/></svg>
                  <h4>{{ t('settings.general.windowLayout') }}</h4>
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
                <div class="setting-item">
                  <label>{{ t('settings.general.layoutManagement') }}</label>
                  <button class="window-reset-btn" :disabled="actionLoading" @click="resetDefaultWindows">
                    {{ t('settings.general.restoreLayout') }}
                  </button>
                  <div class="setting-desc">{{ t('settings.general.restoreLayoutDesc') }}</div>
                </div>
              </div>
            </div>

            <!-- 提示词管理页：列表 + 编辑器 + 拼装预览（独立 tab，即时生效型，无"保存全部"按钮） -->
            <div v-else-if="activeTab === 'prompts'" class="prompts-tab-content">
              <PromptsTab />
            </div>

            <!-- 工具开关页：逐个启停工具。
                 与技能开关不同，改完只对**新会话**生效 —— 核心工具 schema 必须会话内
                 字节恒定，否则 prompt cache 失效。要立刻生效点页内「应用到当前会话」。 -->
            <div v-else-if="activeTab === 'tools'" class="tab-content">
              <ToolsPanel />
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
                  <!-- 明文切换按钮常驻：保存后也能随时核对（以前只有输入时能看到自己刚敲的内容） -->
                  <div class="input-with-toggle">
                    <input
                      :type="showApiKey ? 'text' : 'password'"
                      v-model="editingProfile.config.apiKey"
                      placeholder="sk-..."
                      autocomplete="off"
                      spellcheck="false"
                    />
                    <button
                      type="button"
                      class="toggle-visibility"
                      :class="{ active: showApiKey }"
                      :title="showApiKey ? t('settings.profileEditor.hideApiKey') : t('settings.profileEditor.showApiKey')"
                      :aria-label="showApiKey ? t('settings.profileEditor.hideApiKey') : t('settings.profileEditor.showApiKey')"
                      :aria-pressed="showApiKey"
                      @click="showApiKey = !showApiKey"
                    >
                      <svg v-if="showApiKey" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"></path>
                        <line x1="1" y1="1" x2="23" y2="23"></line>
                      </svg>
                      <svg v-else viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
                        <circle cx="12" cy="12" r="3"></circle>
                      </svg>
                    </button>
                  </div>
                  <div class="setting-desc">{{ t('settings.profileEditor.apiKeyDesc') }}</div>
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
                    <span
                      v-if="editingMainStatus"
                      class="badge"
                      :class="statusBadgeClass(editingMainStatus)"
                    >{{ statusLabel(editingMainStatus) }}</span>
                    <span class="badge" :class="mainModelCaps ? 'badge-ok' : 'badge-none'">
                      <span>{{ mainModelCaps ? t('settings.profileEditor.recognized') : t('settings.profileEditor.unknownModel') }}</span>
                    </span>
                    <template v-if="mainModelCaps">
                      <span v-if="mainModelCaps.thinking" class="badge badge-think">{{ t('settings.profileEditor.thinking') }}</span>
                      <span v-if="mainModelCaps.vision" class="badge badge-ok">{{ t('settings.profileEditor.vision') }}</span>
                      <span class="badge badge-info">{{ t('settings.profileEditor.outputTokens', { count: mainModelCaps.maxTokens.toLocaleString() }) }}</span>
                    </template>
                  </div>
                  <!-- 生命周期提示：退役=红色阻断，即将下线=黄色告警，未能验证/旧名=中性说明 -->
                  <div
                    v-if="editingMainStatus"
                    class="setting-desc model-status-hint"
                    :class="'hint-' + editingMainStatus"
                  >{{ statusHint(editingMainStatus) }}</div>
                  <div class="setting-item" v-if="mainModelCaps !== null">
                    <label>输出上限 (max_tokens)</label>
                    <input
                      type="number"
                      v-model.number="editingProfile.config.maxTokens"
                      :placeholder="String(mainModelCaps?.maxTokens ?? '')"
                      min="1"
                      :max="mainModelCaps?.maxTokens || 32768"
                      step="1"
                    />
                    <div class="setting-desc">留空使用模型默认值（{{ mainModelCaps?.maxTokens?.toLocaleString() ?? '未知' }}），填写则覆盖</div>
                  </div>
                </div>
                <div class="setting-item">
                  <label>{{ t('settings.profileEditor.utilityModel') }}</label>
                  <input type="text" v-model="editingProfile.config.utilityModel" placeholder="claude-3-5-haiku-..." />
                  <!-- 工具代理模型同样受退役拦截约束（徽章紧跟输入框，与主模型版式一致） -->
                  <div class="capability-badges" v-if="editingUtilityStatus">
                    <span class="badge" :class="statusBadgeClass(editingUtilityStatus)">{{ statusLabel(editingUtilityStatus) }}</span>
                  </div>
                  <div
                    v-if="editingUtilityStatus"
                    class="setting-desc model-status-hint"
                    :class="'hint-' + editingUtilityStatus"
                  >{{ statusHint(editingUtilityStatus) }}</div>
                  <div class="setting-desc">{{ t('settings.profileEditor.utilityModelDesc') }}</div>
                </div>

                <!--
                  深度思考的默认档位曾放在这里（预设级）。已迁到「常规设置」：
                  预设级会让"改一个预设"倒灌所有未表态会话（NULL 每轮现读该预设），
                  而它本质是个**全局默认值**，与预设的采样参数不是同一层概念。
                -->

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

      <!--
        退役阻断提示：必须常驻可见。
        保存按钮被置灰时点不动，`save()` 里那句报错永远不会显示——
        用户只会看到"保存是灰的"，却不知道为什么。
      -->
      <div
        v-if="activeTab === 'presets' && retiredBlockLines.length > 0"
        class="retired-block-notice"
        role="alert"
      >
        <div v-for="(line, i) in retiredBlockLines" :key="i" class="retired-block-line">{{ line }}</div>
      </div>

      <div class="settings-footer">
        <span class="status-msg" :class="{ 'error': isError, 'success': isSuccess }">{{ statusMsg }}</span>
        <div class="footer-actions">
          <!-- 有**本次新引入的**退役模型时置灰：点得动再报错不如一开始就不让点 -->
          <button v-if="activeTab === 'presets'" class="save-btn" @click="save" :disabled="isSaving || actionLoading || retiredBlockers.length > 0">
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
import {
  usePreferences,
  FONT_SIZE_PRESETS,
  CODE_FONT_SIZE_PRESETS,
  type AgentPanelPosition,
  type ImageCompressTier,
} from '../../composables/usePreferences'
import { useWindow } from '../../composables/useWindow'
import type { AgentUserMode } from '../../types'
import type { ThinkingDefault } from '../../utils/thinking'
import ConfirmModal from '../common/ConfirmModal.vue'
import PromptsTab from './PromptsTab.vue'
import ToolsPanel from './ToolsPanel.vue'

const { t, locale } = useI18n()

const { isDark, toggleTheme } = useTheme()
const uiPrefs = usePreferences()
const { resetWindowStates, notifyMonitorLocaleChanged } = useWindow()
const agentAudience = uiPrefs.agentAudience
const setAgentAudience = (val: "user" | "developer") => uiPrefs.setAgentAudience(val)
const agentWorkMode = uiPrefs.agentWorkMode
const agentApprovalMode = uiPrefs.agentApprovalMode
// 深度思考的**设置默认档位**（三值）。与其他设置默认值同理：只喂新建会话，
// 「跟随全局」的解析在后端按模型能力完成，前端不做推导。
const thinkingDefault = uiPrefs.thinkingDefault
const setThinkingDefault = (val: ThinkingDefault) => uiPrefs.setThinkingDefault(val)

/**
 * 设置页只写**默认值**，刻意不碰任何会话。
 *
 * 这三个值是三条独立通道：设置默认值 / 会话界面值 / session 表值。设置页改的是
 * "以后新会话用什么"，若顺手把当前会话也改了（旧行为），用户就丧失了"只改默认"的
 * 能力——而且当前会话本应由用户在该会话界面里自己拨。会话值的写入时机统一在
 * 发送消息时（`chat.ts ensureActiveSessionForSend` 消费 pending 落库）。
 */
const setAgentWorkMode = (val: AgentUserMode) => {
  uiPrefs.setAgentWorkMode(val)
}

/** 权限档位：请求审批（改动一律问）/ 帮我批准（只问风险操作） */
const setAgentApprovalMode = (val: "request_approval" | "auto_approve") => {
  uiPrefs.setAgentApprovalMode(val)
}
const fontSize = computed(() => uiPrefs.fontSize)
const setFontSize = (val: number) => uiPrefs.setFontSize(val)
const codeFontSize = computed(() => uiPrefs.codeFontSize)
const setCodeFontSize = (val: number) => uiPrefs.setCodeFontSize(val)
// 字号是离散偏好而非连续量：四档预设替代滑杆。
// 落库的值一律在 normalizePrefs 里吸附到最近挡位，所以不存在「一个都不高亮」的状态
// —— 旧版默认 15 落在旧挡位 12/14/16/18 之外，曾长期卡在那个死角：
// 首屏四档全不亮，而 setFontSize 只有这四个按钮一个入口，点过一次就回不到默认值。
// 挡位数值一律取自 usePreferences 的唯一出处，这里只负责挂文案。
// 不能就地另抄一份：按钮与 normalizePrefs 的吸附口径一旦漂移，
// 就会出现「点了某个挡位、落库后被吸附到另一个挡位」的鬼故事。
const fontSizePresets = [
  { value: FONT_SIZE_PRESETS[0], label: 'settings.general.sizeS' },
  { value: FONT_SIZE_PRESETS[1], label: 'settings.general.sizeM' },
  { value: FONT_SIZE_PRESETS[2], label: 'settings.general.sizeL' },
  { value: FONT_SIZE_PRESETS[3], label: 'settings.general.sizeXL' },
]
const codeFontSizePresets = [
  { value: CODE_FONT_SIZE_PRESETS[0], label: 'settings.general.sizeS' },
  { value: CODE_FONT_SIZE_PRESETS[1], label: 'settings.general.sizeM' },
  { value: CODE_FONT_SIZE_PRESETS[2], label: 'settings.general.sizeL' },
  { value: CODE_FONT_SIZE_PRESETS[3], label: 'settings.general.sizeXL' },
]
const defaultExpandThinking = computed(() => uiPrefs.defaultExpandThinking)
const setDefaultExpandThinking = (val: boolean) => uiPrefs.setDefaultExpandThinking(val)
const autoScroll = computed(() => uiPrefs.autoScroll)
const setAutoScroll = (val: boolean) => uiPrefs.setAutoScroll(val)
// 崩溃保护（实时保存）：纯后端口径开关，前端只负责落到偏好里
const crashProtection = computed(() => uiPrefs.crashProtection)
const setCrashProtection = (val: boolean) => uiPrefs.setCrashProtection(val)
const agentPanelPosition = computed(() => uiPrefs.agentPanelPosition)
const setAgentPanelPosition = (val: AgentPanelPosition) => uiPrefs.setAgentPanelPosition(val)
const compactMode = computed(() => uiPrefs.compactMode)
const setCompactMode = (val: boolean) => uiPrefs.setCompactMode(val)
const agentMessageOpacity = computed(() => uiPrefs.agentMessageOpacity)
const setAgentMessageOpacity = (val: number) => uiPrefs.setAgentMessageOpacity(val)
const userMessageOpacity = computed(() => uiPrefs.userMessageOpacity)
const setUserMessageOpacity = (val: number) => uiPrefs.setUserMessageOpacity(val)
const reflectionMode = computed(() => uiPrefs.reflectionMode)
const setReflectionMode = (val: "always" | "smart" | "off") => uiPrefs.setReflectionMode(val)
/**
 * 图片压缩三档。`title` 是**悬停用**的一句话说明（什么时候该选这一档），
 * 下方的 setting-desc 则是当前档位的展开描述 —— 两者分工不同，不要合并。
 */
const imageCompressTiers: {
  value: ImageCompressTier
  label: string
  tooltip: string
}[] = [
  { value: 'eco', label: 'settings.general.imageCompressTierEco', tooltip: 'settings.general.imageCompressTierEcoTooltip' },
  { value: 'standard', label: 'settings.general.imageCompressTierStandard', tooltip: 'settings.general.imageCompressTierStandardTooltip' },
  { value: 'hd', label: 'settings.general.imageCompressTierHd', tooltip: 'settings.general.imageCompressTierHdTooltip' },
]
const imageCompressTier = computed(() => uiPrefs.imageCompressTier)
const setImageCompressTier = (val: ImageCompressTier) => uiPrefs.setImageCompressTier(val)
const langMenuOpen = ref(false)
const reflectionMenuOpen = ref(false)
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
const activeTab = ref<'general' | 'presets' | 'prompts' | 'tools'>('general')
const showAdvanced = ref(false)

interface AgentConfig {
  apiFormat: string
  apiKey: string
  baseUrl: string
  mainModel: string
  utilityModel: string
  temperature?: number | null
  topP?: number | null
  topK?: number | null
  maxTokens?: number | null
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
  /**
   * 模型生命周期状态（后端 `ModelCapabilities.status`，注册表省略即 `active`）：
   * - `active`       在售
   * - `deprecated`   官方已公告即将下线，**当前仍可调用**
   * - `retired`      官方已宣布退役，请求必然失败 → 预设禁止保存
   * - `unverifiable` 官方当前目录查不到，**但无退役公告**（不等于 retired）
   * - `alias`        旧名，请求被路由到新模型
   */
  status?: string
  /**
   * 状态补充说明（**只放事实**：何时退役、依据哪份官方公告）。
   *
   * 刻意不含"建议换成哪个型号"——建议会被时间淘汰（被建议的型号自己也会退役），
   * 界面统一引导用户去厂商官方文档查最新型号。
   */
  statusNote?: string
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
    temperature: null,
    topP: null,
    topK: null,
    maxTokens: null,
  }
})

const cloneConfig = <T>(value: T): T => JSON.parse(JSON.stringify(value))

const normalizeProfileConfig = (config: AppConfig) => {
  config.profiles.forEach((p) => {
    p.config.temperature = p.config.temperature == null ? null : Number(p.config.temperature)
    p.config.topP = p.config.topP == null ? null : Number(p.config.topP)
    p.config.topK = p.config.topK == null ? null : Number(p.config.topK)
    p.config.maxTokens = p.config.maxTokens == null ? null : Number(p.config.maxTokens)
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

// ── 模型生命周期状态（退役 / 即将下线 / 未能验证 / 旧名路由）──
//
// 为什么需要一份"所有草稿模型"的状态缓存，而不是只看当前编辑预设的 mainModelCaps：
// 保存时会遍历**全部**预设，任一预设**本次新引入**了退役模型都要拦下
// （判据见 retiredHitsIn：与已保存状态比对，既有状态不算）。只探测当前预设的话，
// 切到另一个预设点保存就会漏判。
//
// 只缓存 `status`：注册表刻意不记录"建议换成哪个型号"（那会被时间淘汰，
// 写死就是持续维护负担），所以这里没有可缓存的迁移目标。

/** 单个模型的生命周期信息；`status === ''` 表示注册表未收录（我们不知道） */
interface ModelLifecycle {
  status: string
}

const modelLifecycleCache = ref<Record<string, ModelLifecycle>>({})

/** 读缓存；未收录返回 `{ status: '' }`，**不要当成 retired** */
const lifecycleOf = (modelId: string): ModelLifecycle =>
  modelLifecycleCache.value[modelId] ?? { status: '' }

const cacheLifecycle = (modelId: string, caps: ModelCapabilities | null) => {
  modelLifecycleCache.value = {
    ...modelLifecycleCache.value,
    [modelId]: { status: caps?.status ?? '' },
  }
}

/** 探测一批模型 id 的状态；已缓存的跳过（本地读内嵌注册表，开销极小） */
const probeModelLifecycles = async (ids: string[]) => {
  const missing = [...new Set(ids.map(id => id.trim()).filter(Boolean))]
    .filter(id => !(id in modelLifecycleCache.value))
  if (missing.length === 0) return

  const results = await Promise.all(
    missing.map(async (id) => {
      try {
        const caps = await invoke<ModelCapabilities | null>('get_model_capabilities', { modelId: id })
        return [id, caps] as const
      } catch {
        return [id, null] as const
      }
    }),
  )

  const next = { ...modelLifecycleCache.value }
  for (const [id, caps] of results) {
    next[id] = { status: caps?.status ?? '' }
  }
  modelLifecycleCache.value = next
}

/** 草稿里被引用的全部模型 id（主模型 + 工具代理模型，跨所有预设） */
const draftModelIds = computed(() => {
  const ids: string[] = []
  for (const p of draftConfig.value.profiles) {
    const main = p.config.mainModel?.trim()
    const utility = p.config.utilityModel?.trim()
    if (main) ids.push(main)
    if (utility) ids.push(utility)
  }
  return ids
})

/**
 * 本次草稿里**新引入**的退役模型；非空即禁止保存。
 *
 * 判据是"与**已保存状态**比对"，而不是"草稿里存在退役模型"：
 *
 * - 已保存状态里就是这个退役型号、草稿没动它 → **不算**（那是既有状态，不是用户刚配的）
 * - 用户把模型改成了退役型号 → 算
 * - 新建预设里写了退役型号 → 算（`saved` 找不到，`before` 为空串）
 *
 * 为什么必须这么判：默认预设用的就是 `mimo-v2-flash`（已退役）。若按"存在即拦"，
 * 新装用户打开设置改任何字段都会发现保存按钮是灰的，而且**永远存不进去**。
 * 另外，A、B 两个预设都含退役型号时，只修 A 也该能保存——B 不该拖住 A。
 */
/** 挡住保存的一处：哪个预设的哪个字段、写的哪个退役型号 */
interface RetiredHit {
  profileName: string
  field: 'mainModel' | 'utilityModel'
  modelId: string
}

const retiredHitsIn = (profiles: ModelProfile[]): RetiredHit[] => {
  const hits: RetiredHit[] = []
  for (const p of profiles) {
    const saved = savedConfig.value.profiles.find(s => s.id === p.id)
    for (const field of ['mainModel', 'utilityModel'] as const) {
      const id = p.config[field]?.trim()
      if (!id || lifecycleOf(id).status !== 'retired') continue
      // 与已保存值一致 → 既有状态，放行
      if (saved?.config?.[field]?.trim() === id) continue
      hits.push({ profileName: p.name, field, modelId: id })
    }
  }
  return hits
}

const retiredBlockers = computed(() => retiredHitsIn(draftConfig.value.profiles))

/**
 * 阻断原因摘要（逐条列出）。
 *
 * 保存按钮被置灰时，光靠"点击报错"是没用的——**按钮点不动，报错永远看不到**。
 * 所以必须在界面上常驻一条可见提示，并且要列全，不能只说第一个。
 */
const retiredBlockLines = computed(() => retiredBlockers.value.map(hit => retiredBlockMessage(hit)))

let lifecycleProbeTimer: ReturnType<typeof setTimeout> | null = null
watch(draftModelIds, (ids) => {
  if (lifecycleProbeTimer) clearTimeout(lifecycleProbeTimer)
  lifecycleProbeTimer = setTimeout(() => { void probeModelLifecycles(ids) }, 300)
}, { immediate: true })

/** 当前编辑预设的模型状态（驱动徽章与提示文案） */
const editingMainStatus = computed(() => {
  const id = editingProfile.value?.config.mainModel?.trim()
  return id ? lifecycleOf(id).status : ''
})
const editingUtilityStatus = computed(() => {
  const id = editingProfile.value?.config.utilityModel?.trim()
  return id ? lifecycleOf(id).status : ''
})

const statusBadgeClass = (status: string) => {
  switch (status) {
    case 'retired': return 'badge-retired'
    case 'deprecated': return 'badge-deprecated'
    case 'unverifiable': return 'badge-unverifiable'
    case 'alias': return 'badge-info'
    default: return ''
  }
}

const statusLabel = (status: string) => {
  switch (status) {
    case 'retired': return t('settings.profileEditor.statusRetired')
    case 'deprecated': return t('settings.profileEditor.statusDeprecated')
    case 'unverifiable': return t('settings.profileEditor.statusUnverifiable')
    case 'alias': return t('settings.profileEditor.statusAlias')
    default: return ''
  }
}

/**
 * 状态提示文案。
 *
 * **刻意不写"建议换成 X"**：那个 X 会被时间淘汰（它自己以后也可能退役），
 * 写死就得一直维护，且过期后从"有用"变成"误导"。统一引导去厂商官方文档。
 */
const statusHint = (status: string) => {
  switch (status) {
    case 'retired': return t('settings.profileEditor.retiredHint')
    case 'deprecated': return t('settings.profileEditor.deprecatedHint')
    case 'unverifiable': return t('settings.profileEditor.unverifiableHint')
    case 'alias': return t('settings.profileEditor.aliasHint')
    default: return ''
  }
}

/** 退役拦截的提示文案（保存时用） */
const retiredBlockMessage = (hit: RetiredHit) => {
  const key = hit.field === 'mainModel'
    ? 'settings.validation.mainModelRetired'
    : 'settings.validation.utilityModelRetired'
  return t(key, { name: hit.profileName, model: hit.modelId })
}

/** API Key 明文开关：常驻可切，切换 profile 时自动回到隐藏 */
const showApiKey = ref(false)

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
      // 同步进生命周期缓存：徽章与保存拦截共用同一份真相，避免两处结论打架
      cacheLifecycle(modelId, caps)
    } catch {
      mainModelCaps.value = null
      cacheLifecycle(modelId, null)
    }
  }, 400)
}

watch(selectedProfileId, () => {
  showApiKey.value = false // 切档案时回到隐藏，避免上一个档案的明文状态带过去
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
  if (e.target instanceof HTMLElement && !e.target.closest('.custom-select')) {
    if (langMenuOpen.value) langMenuOpen.value = false
    if (reflectionMenuOpen.value) reflectionMenuOpen.value = false
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

  const contentKeys: Array<keyof AgentConfig> = [
    'apiKey',
    'baseUrl',
    'mainModel',
    'utilityModel',
    'temperature',
    'topP',
    'topK',
    'maxTokens',
  ]

  return contentKeys.some((key) => hasMeaningfulDraftValue(profile.config[key]))
}

const persistFilledNewProfilesBeforeClose = async () => {
  const savedIds = new Set(savedConfig.value.profiles.map((profile) => profile.id))
  const filledNewProfiles = draftConfig.value.profiles.filter((profile) => {
    return !savedIds.has(profile.id) && hasNewProfileContent(profile)
  })

  if (filledNewProfiles.length === 0) return

  // 关窗路径会**自动**保存"已填写的新预设"——只拦保存按钮会被这里绕过。
  //
  // 只检查即将写入的这一批：已保存的预设本次不会被重写（close() 会用 savedConfig
  // 覆盖草稿），把它们也算进来会让用户因为一个没在改的旧预设而关不掉设置面板。
  await probeModelLifecycles(
    filledNewProfiles.flatMap(p => [p.config.mainModel ?? '', p.config.utilityModel ?? '']),
  )
  const hits = retiredHitsIn(filledNewProfiles)
  if (hits.length > 0) {
    throw new Error(retiredBlockMessage(hits[0]))
  }

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

  // 退役模型拦截。先补一次探测：缓存有 300ms 防抖，用户刚敲完就点保存时可能还没落缓存。
  await probeModelLifecycles(draftModelIds.value)
  if (retiredBlockers.value.length > 0) {
    setErrorStatus(retiredBlockMessage(retiredBlockers.value[0]))
    return
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
  /* 选中态实心底：小面积开关用纯 --text-main 视觉刚好，大面积实心会被衬得更黑，
     故混入更多底色（76/24）让两者看起来是同一种"黑" */
  --sel-fill: color-mix(in srgb, var(--text-main) 76%, var(--surface-strong));
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
  box-shadow: var(--shadow-lg);
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
  color: var(--text-main);
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
  font-size: 0.9333rem;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.nav-item:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.nav-item.active {
  background: color-mix(in srgb, var(--text-main) 10%, transparent);
  color: var(--text-main);
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
  color: var(--text-main);
  border: 1px solid color-mix(in srgb, var(--text-main) 25%, transparent);
  width: 20px;
  height: 20px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  font-size: 0.9333rem;
  transition: all var(--transition-fast);
}

.add-btn:hover {
  background: color-mix(in srgb, var(--text-main) 10%, transparent);
  border-color: var(--text-main);
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
  font-size: 0.8667rem;
}

.profile-item:hover {
  background: var(--glass-bg-light);
  color: var(--text-main);
}

.profile-item.active {
  background: color-mix(in srgb, var(--text-main) 8%, transparent);
  color: var(--text-main);
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

.sidebar-switch input:checked + .slider { background-color: var(--sel-fill); }
.sidebar-switch input:checked + .slider:before { transform: translateX(10px); background-color: var(--surface-strong); }

.copy-btn, .delete-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  opacity: 0;
  cursor: pointer;
  padding: 2px;
}

.profile-item:hover .copy-btn, .profile-item:hover .delete-btn { opacity: 1; }
.copy-btn:hover { color: var(--text-main); }
.delete-btn:hover { color: var(--accent-red); }

.profile-item { cursor: grab; }
.profile-item.dragging { opacity: 0.5; cursor: grabbing; }
.profile-item.drag-over {
  border-top: 2px solid var(--text-main);
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

/* 提示词 tab 自带滚动（textarea 内滚），外层锁高度让编辑器占满可视区 */
.settings-body.no-scroll {
  overflow-y: hidden;
  display: flex;
  flex-direction: column;
}

.tab-content {
  display: flex;
  flex-direction: column;
  gap: 20px;
  max-width: 800px;
  margin: 0 auto;
}

/* 提示词 tab：占满整个内容区（编辑器要纵向空间），弹层以内容区为定位容器 */
.prompts-tab-content {
  position: relative;
  height: 100%;
  min-height: 0;
  max-width: 1100px;
  margin: 0 auto;
  width: 100%;
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
  font-size: 1rem;
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
  font-size: 0.8667rem;
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
  font-size: 0.8667rem;
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
  border-color: var(--text-soft);
  background-color: var(--glass-bg);
}

.format-select {
  width: auto !important;
  min-width: 160px;
}

/* API Key：常驻明文切换按钮（右侧内嵌，不挤压输入区） */
.input-with-toggle {
  position: relative;
  display: flex;
  align-items: center;
}

.input-with-toggle input {
  padding-right: 40px;
}

.toggle-visibility {
  position: absolute;
  right: 6px;
  width: 28px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: color 0.15s, background-color 0.15s;
}

.toggle-visibility:hover {
  color: var(--text-main);
  background-color: var(--glass-bg);
}

.toggle-visibility.active {
  color: var(--text-main);
}

/* ── 自定义下拉（替代原生 select）── */
.custom-select {
  position: relative;
  width: fit-content;
}

.custom-select-trigger {
  width: 100%;
  height: 34px;
  padding: 0 10px;
  padding-right: 28px;
  background-color: var(--glass-bg-light);
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--text-main);
  font-size: 0.8667rem;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  cursor: pointer;
  transition: border-color 0.2s, box-shadow 0.2s, background-color 0.2s;
  white-space: nowrap;
}

.custom-select-trigger:hover {
  border-color: var(--text-soft);
  background-color: var(--glass-bg);
}

.custom-select.open .custom-select-trigger {
  border-color: var(--text-main);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--text-main) 10%, transparent);
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
  min-width: 100%;
  z-index: 110;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 20%, transparent);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(10, 10, 10, 0.12);
  overflow: hidden;
  padding: 4px;
  white-space: nowrap;
  animation: popIn var(--transition-fast);
}

.custom-select-option {
  padding: 8px 12px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 0.8667rem;
  color: var(--text-main);
  transition: background-color 0.15s;
}

.custom-select-option:hover {
  background: color-mix(in srgb, var(--text-main) 8%, transparent);
}

.custom-select-option.active {
  background: color-mix(in srgb, var(--text-main) 14%, transparent);
  color: var(--text-main);
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
  border-color: var(--text-main);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--text-main) 10%, transparent);
}

.setting-desc {
  font-size: 0.8rem;
  color: var(--text-muted);
  line-height: 1.5;
}

/* 字体大小控件 */
.font-size-control {
  display: flex;
  align-items: center;
  gap: 10px;
}

.slider-track-wrap {
  flex: 1;
  max-width: 180px;
  display: flex;
  align-items: center;
}

/* 透明度滑杆（现仅消息透明度两行在用）：真连续量，保留滑杆形式，整体瘦身降存在感 */
.font-size-slider {
  width: 100%;
  height: 16px;
  -webkit-appearance: none;
  appearance: none;
  background: transparent;
  outline: none;
  cursor: pointer;
  margin: 0;
}

/* 轨道 */
.font-size-slider::-webkit-slider-runnable-track {
  height: 4px;
  border-radius: 2px;
  background: linear-gradient(
    to right,
    var(--sel-fill) 0%,
    var(--sel-fill) var(--fill-pct, 50%),
    var(--glass-bg-light) var(--fill-pct, 50%),
    var(--glass-bg-light) 100%
  );
  border: 0.5px solid var(--glass-border-subtle);
}

/* 滑块钮 */
.font-size-slider::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--surface-strong);
  border: 2px solid var(--sel-fill);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.12);
  margin-top: -5px;
  cursor: pointer;
  transition: box-shadow 0.2s ease, transform 0.15s ease;
}

.font-size-slider::-webkit-slider-thumb:hover {
  box-shadow: 0 1px 6px rgba(0, 0, 0, 0.2);
}

.font-size-slider::-webkit-slider-thumb:active {
  transform: scale(0.92);
}

/* Firefox */
.font-size-slider::-moz-range-track {
  height: 4px;
  border-radius: 2px;
  background: var(--glass-bg-light);
  border: 0.5px solid var(--glass-border-subtle);
}

.font-size-slider::-moz-range-progress {
  height: 4px;
  border-radius: 2px;
  background: var(--sel-fill);
}

.font-size-slider::-moz-range-thumb {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--surface-strong);
  border: 2px solid var(--sel-fill);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.12);
  cursor: pointer;
}

.font-size-value {
  font-size: 0.8rem;
  font-weight: var(--fw-semibold);
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
  background: var(--sel-fill);
  border-color: var(--sel-fill);
}

.toggle-switch input:checked + .toggle-slider::before {
  transform: translateX(20px);
  background: var(--surface-strong);
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
  font-size: 0.8667rem;
  font-weight: 600;
  color: var(--text-muted);
  cursor: pointer;
  transition: all 0.2s;
}

.display-mode-btn.active {
  background: var(--sel-fill);
  color: var(--surface-strong);
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
  font-size: 0.8667rem;
  cursor: pointer;
}

.window-reset-btn {
  width: fit-content;
  padding: 8px 16px;
  background: transparent;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  color: var(--accent-red);
  font-size: 0.8667rem;
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
  color: var(--text-main);
  font-size: 0.8667rem;
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
  font-size: 0.7333rem;
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
  font-size: 0.7333rem;
  font-weight: 600;
}

.badge-ok { background: color-mix(in srgb, var(--accent-green) 10%, transparent); color: var(--accent-green); }
.badge-think { background: color-mix(in srgb, var(--text-muted) 12%, transparent); color: var(--text-soft); }
.badge-info { background: color-mix(in srgb, var(--text-main) 10%, transparent); color: var(--text-main); }
.badge-none { background: color-mix(in srgb, var(--text-muted) 10%, transparent); color: var(--text-muted); }

/* ── 模型生命周期徽章 ──
   retired 用红色强调：它是唯一会阻断保存的状态。
   deprecated 用琥珀色告警（尚未生效）。
   unverifiable 刻意**不用红色**，改用灰色 + 虚边表达"我们不确定"——
   它和 retired 不是一回事（见 registry.rs 里 status 字段的文档：
   查不到 ≠ 已退役，同 thinking.ts 的「未知 ≠ 不支持」）。 */
.badge-retired {
  background: color-mix(in srgb, var(--accent-red) 14%, transparent);
  color: var(--accent-red);
  border: 1px solid color-mix(in srgb, var(--accent-red) 35%, transparent);
}

.badge-deprecated {
  background: color-mix(in srgb, #d97706 14%, transparent);
  color: #b45309;
  border: 1px solid color-mix(in srgb, #d97706 35%, transparent);
}

body.dark-mode .badge-deprecated { color: #fbbf24; }

.badge-unverifiable {
  /* 原为 rgba(100, 116, 139, …) —— 那是 slate-500 的 alpha 写法，带蓝味；
     上一轮去蓝只搜了 rgba(15,23,42) 与十六进制，漏掉了这种形式。
     这里改用与相邻徽章同源的 --text-muted（现已中性化），保持「同色系不同浓度」的层级。 */
  background: color-mix(in srgb, var(--text-muted) 8%, transparent);
  color: var(--text-muted);
  border: 1px dashed color-mix(in srgb, var(--text-muted) 35%, transparent);
}

/* 生命周期提示行：与徽章同色系但更弱，避免和"能力徽章"抢注意力 */
.model-status-hint {
  margin-top: 4px;
  line-height: 1.5;
}
.model-status-hint.hint-retired { color: var(--accent-red); }
.model-status-hint.hint-deprecated { color: #b45309; }
body.dark-mode .model-status-hint.hint-deprecated { color: #fbbf24; }
.model-status-hint.hint-unverifiable,
.model-status-hint.hint-alias { color: var(--text-muted); }

/* 退役阻断提示条：贴在 footer 上方，按钮置灰时唯一能解释原因的地方 */
.retired-block-notice {
  padding: 8px 14px;
  background: color-mix(in srgb, var(--accent-red) 8%, transparent);
  border-top: 1px solid color-mix(in srgb, var(--accent-red) 25%, transparent);
  color: var(--accent-red);
  font-size: 0.8rem;
  line-height: 1.6;
}
.retired-block-line + .retired-block-line { margin-top: 4px; }

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
  background: var(--sel-fill);
  color: var(--surface-strong);
  border: none;
  padding: 10px 24px;
  border-radius: 8px;
  font-size: 0.9333rem;
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
  font-size: 0.8667rem;
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

