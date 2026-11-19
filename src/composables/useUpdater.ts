/**
 * # useUpdater.ts — 应用内更新状态（单例）
 *
 * 基于 tauri-plugin-updater：检查更新清单（latest.json）→ 校验签名 → 下载并安装 → 重启。
 * 状态放模块级单例，App 的更新弹窗与「设置-关于」共享同一份，避免两处各查一次。
 *
 * ## 约束
 * - 开发模式（tauri dev）不检查更新：安装器无法正确替换 dev 进程；
 * - 自动检查失败保持静默（离线启动不该弹错），手动检查才把原因展示出来；
 * - 下载安装完成后不自动重启，由用户点「重启应用」——避免打断进行中的会话。
 */
import { ref } from 'vue';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { getVersion } from '@tauri-apps/api/app';

const isDev = import.meta.env.DEV;

const currentVersion = ref('');
const checking = ref(false);
const update = ref<Update | null>(null);
const upToDate = ref(false);
const downloading = ref(false);
const installed = ref(false);
/** 0-100；-1 表示服务器未给出总大小，只能显示"正在下载" */
const progress = ref(0);
const error = ref('');
const modalVisible = ref(false);

let dismissedThisRun = false;
let versionLoaded: Promise<void> | null = null;

async function init(): Promise<void> {
  if (!versionLoaded) {
    versionLoaded = (async () => {
      try {
        currentVersion.value = await getVersion();
      } catch {
        // 拿不到版本号不影响更新流程，界面按空值处理
        currentVersion.value = '';
      }
    })();
  }
  return versionLoaded;
}

/** 检查更新。manual=true 时反馈"已是最新/失败原因"，自动检查完全静默。 */
async function checkForUpdate(manual = false): Promise<void> {
  if (checking.value || downloading.value) return;
  error.value = '';
  upToDate.value = false;
  if (isDev) {
    if (manual) error.value = 'dev';
    return;
  }
  checking.value = true;
  try {
    const found = await check();
    if (found) {
      update.value = found;
      if (!dismissedThisRun) modalVisible.value = true;
    } else {
      update.value = null;
      if (manual) upToDate.value = true;
    }
  } catch (e) {
    if (manual) error.value = String(e);
  } finally {
    checking.value = false;
  }
}

/** 下载并安装（Windows 下会静默运行 NSIS 安装器）。完成后需要重启生效。 */
async function install(): Promise<void> {
  if (!update.value || downloading.value) return;
  downloading.value = true;
  installed.value = false;
  error.value = '';
  progress.value = 0;
  let total: number | null = null;
  let downloaded = 0;
  try {
    await update.value.downloadAndInstall((event) => {
      switch (event.event) {
        case 'Started':
          total = event.data.contentLength ?? null;
          break;
        case 'Progress':
          downloaded += event.data.chunkLength;
          progress.value =
            total && total > 0
              ? Math.min(100, Math.round((downloaded / total) * 100))
              : -1;
          break;
        case 'Finished':
          progress.value = 100;
          break;
        default:
          break;
      }
    });
    installed.value = true;
  } catch (e) {
    error.value = String(e);
  } finally {
    downloading.value = false;
  }
}

async function restart(): Promise<void> {
  await relaunch();
}

function dismiss(): void {
  modalVisible.value = false;
  dismissedThisRun = true;
}

/** 用户从设置页主动查看更新详情 */
function showModal(): void {
  if (update.value) modalVisible.value = true;
}

export function useUpdater() {
  return {
    isDev,
    currentVersion,
    checking,
    update,
    upToDate,
    downloading,
    installed,
    progress,
    error,
    modalVisible,
    init,
    checkForUpdate,
    install,
    restart,
    dismiss,
    showModal,
  };
}
