/**
 * # useMediaAttachments.ts — 输入框图片/媒体附件管理与压缩管线
 *
 * 封装拖拽文件与剪贴板粘贴图片的解码缩放（`OffscreenCanvas`）、token 估算、
 * 悬停提示文案、非视觉模型警告以及 ObjectURL 生命周期清理。
 *
 * ## Key Exports
 * - `MediaFile`: 待发送附件元信息与 base64 数据
 * - `estimateImageTokens()`: 按 Anthropic 官方 `(w * h) / 750` 估算图像 token
 * - `useMediaAttachments()`: 管理附件列表、拖拽/粘贴处理与资源释放
 *
 * ## Dependencies
 * - Internal: `@/composables/usePreferences`
 * - External: `@tauri-apps/plugin-fs`, `vue`, `vue-i18n`
 */
import { computed, nextTick, ref, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import { readFile } from "@tauri-apps/plugin-fs";
import { usePreferences } from "../usePreferences";

/** 待发送附件（`natural*` / `compressed*` 仅用于悬停提示，不参与发送） */
export interface MediaFile {
  path: string;
  type: "image" | "video";
  url: string;
  base64: string;
  naturalWidth: number | null;
  naturalHeight: number | null;
  compressedWidth: number | null;
  compressedHeight: number | null;
}

/** 估算压缩后图像的大致 token 数（仅展示用，不参与请求构造） */
export function estimateImageTokens(w: number | null, h: number | null): number | null {
  if (!w || !h) return null;
  return Math.round((w * h) / 750);
}

/** 从文件名推断图片 MIME（兜底 WebView2 DataTransferItem.type 为空的场景） */
function inferImageMime(name: string): string | null {
  const ext = name.split(".").pop()?.toLowerCase() || "";
  if (ext === "jpg" || ext === "jpeg") return "image/jpeg";
  if (ext === "png") return "image/png";
  if (ext === "gif") return "image/gif";
  if (ext === "webp") return "image/webp";
  return null;
}

interface MediaAttachmentsOptions {
  canModelVision: Ref<boolean>;
  userInput: Ref<string>;
  inputRef: Ref<HTMLTextAreaElement | null>;
  adjustHeight: () => void;
}

export function useMediaAttachments(options: MediaAttachmentsOptions) {
  const { canModelVision, userInput, inputRef, adjustHeight } = options;
  const { t } = useI18n();
  const uiPrefs = usePreferences();

  const mediaFiles = ref<MediaFile[]>([]);
  const showVisionWarning = ref(false);

  const imageCompressConfig = computed(() => ({
    maxWidth: uiPrefs.imageMaxWidth,
    maxHeight: uiPrefs.imageMaxHeight,
    quality: uiPrefs.imageQuality,
  }));

  const compressImage = async (
    fileData: Uint8Array,
    mimeType: string,
  ): Promise<{
    base64: string;
    mimeType: string;
    naturalWidth: number;
    naturalHeight: number;
    compressedWidth: number;
    compressedHeight: number;
  }> => {
    const { maxWidth, maxHeight, quality } = imageCompressConfig.value;
    const blob = new Blob([new Uint8Array(fileData)], { type: mimeType });
    const bitmap = await createImageBitmap(blob);

    const naturalWidth = bitmap.width;
    const naturalHeight = bitmap.height;
    let w = naturalWidth;
    let h = naturalHeight;

    if (w > maxWidth || h > maxHeight) {
      const ratio = Math.min(maxWidth / w, maxHeight / h);
      w = Math.round(w * ratio);
      h = Math.round(h * ratio);
    }

    const canvas = new OffscreenCanvas(w, h);
    const ctx = canvas.getContext("2d")!;
    ctx.drawImage(bitmap, 0, 0, w, h);
    bitmap.close();

    const outputType = mimeType === "image/png" ? "image/png" : "image/jpeg";
    const outputBlob = await canvas.convertToBlob({ type: outputType, quality });
    const reader = new FileReader();

    return new Promise((resolve) => {
      reader.onload = () => {
        const dataUrl = reader.result as string;
        resolve({
          base64: dataUrl,
          mimeType: outputType,
          naturalWidth,
          naturalHeight,
          compressedWidth: w,
          compressedHeight: h,
        });
      };
      reader.readAsDataURL(outputBlob);
    });
  };

  const mediaTooltip = (media: MediaFile): string => {
    const lines: string[] = [media.path.split(/[/\\]/).pop() || media.path];

    if (media.naturalWidth && media.naturalHeight) {
      let sizeLine = `${media.naturalWidth}×${media.naturalHeight}`;
      if (
        media.compressedWidth &&
        media.compressedHeight &&
        (media.compressedWidth !== media.naturalWidth ||
          media.compressedHeight !== media.naturalHeight)
      ) {
        sizeLine += ` → ${media.compressedWidth}×${media.compressedHeight}`;
      } else {
        sizeLine += ` → ${t("input.imageNotCompressed")}`;
      }
      lines.push(sizeLine);
    }

    const tokens = estimateImageTokens(media.compressedWidth, media.compressedHeight);
    if (tokens !== null) {
      lines.push(t("input.imageTokenEstimate", { count: tokens.toLocaleString() }));
    }

    if (media.type === "video") {
      lines.push(t("input.videoTokenUnknown"));
    }

    return lines.join("\n");
  };

  const processDroppedFiles = async (paths: string[]) => {
    let hasMediaFiles = false;

    for (const droppedPath of paths) {
      const lowerPath = droppedPath.toLowerCase();
      const isImage =
        lowerPath.endsWith(".jpg") ||
        lowerPath.endsWith(".jpeg") ||
        lowerPath.endsWith(".png") ||
        lowerPath.endsWith(".gif") ||
        lowerPath.endsWith(".webp");
      const isVideo =
        lowerPath.endsWith(".mp4") ||
        lowerPath.endsWith(".webm") ||
        lowerPath.endsWith(".mov");

      if (isImage || isVideo) {
        hasMediaFiles = true;
        let url = "";
        let base64 = "";
        let naturalWidth: number | null = null;
        let naturalHeight: number | null = null;
        let compressedWidth: number | null = null;
        let compressedHeight: number | null = null;

        if (isImage && canModelVision.value) {
          try {
            const fileData = await readFile(droppedPath);
            const ext = lowerPath.split(".").pop() || "png";
            const mimeType =
              ext === "jpg" || ext === "jpeg"
                ? "image/jpeg"
                : ext === "gif"
                  ? "image/gif"
                  : ext === "webp"
                    ? "image/webp"
                    : "image/png";

            const compressed = await compressImage(new Uint8Array(fileData), mimeType);
            base64 = compressed.base64;
            naturalWidth = compressed.naturalWidth;
            naturalHeight = compressed.naturalHeight;
            compressedWidth = compressed.compressedWidth;
            compressedHeight = compressed.compressedHeight;
            url = URL.createObjectURL(new Blob([new Uint8Array(fileData)], { type: mimeType }));
          } catch (e) {
            console.error("Failed to read image file:", e);
          }
        }

        mediaFiles.value.push({
          path: droppedPath,
          type: isImage ? "image" : "video",
          url,
          base64,
          naturalWidth,
          naturalHeight,
          compressedWidth,
          compressedHeight,
        });
      } else {
        if (userInput.value) {
          userInput.value += ` ${droppedPath}`;
        } else {
          userInput.value = droppedPath;
        }
      }
    }

    if (hasMediaFiles && !canModelVision.value) {
      showVisionWarning.value = true;
    }

    nextTick(() => {
      inputRef.value?.focus();
      adjustHeight();
    });
  };

  const handlePaste = async (event: ClipboardEvent) => {
    const items = event.clipboardData?.items;
    if (!items) return;

    let file: File | null = null;
    let mimeType = "";
    for (const item of Array.from(items)) {
      if (item.kind !== "file") continue;
      const candidate = item.getAsFile();
      if (!candidate) continue;
      const mime = item.type.startsWith("image/") ? item.type : inferImageMime(candidate.name);
      if (mime) {
        file = candidate;
        mimeType = mime;
        break;
      }
    }
    if (!file) return;

    if (!canModelVision.value) {
      showVisionWarning.value = true;
      return;
    }

    // 必须在事件同步阶段拦截，否则 await 之后默认粘贴已经执行
    event.preventDefault();

    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const compressed = await compressImage(bytes, mimeType);
      const ext = compressed.mimeType === "image/png" ? "png" : "jpg";
      const now = new Date();
      const stamp = [now.getHours(), now.getMinutes(), now.getSeconds()]
        .map((n) => String(n).padStart(2, "0"))
        .join("");
      mediaFiles.value.push({
        path: `粘贴图片-${stamp}.${ext}`,
        type: "image",
        url: URL.createObjectURL(file),
        base64: compressed.base64,
        naturalWidth: compressed.naturalWidth,
        naturalHeight: compressed.naturalHeight,
        compressedWidth: compressed.compressedWidth,
        compressedHeight: compressed.compressedHeight,
      });
    } catch (e) {
      console.error("Failed to paste image:", e);
    }
  };

  const removeMediaFile = (index: number) => {
    const [removed] = mediaFiles.value.splice(index, 1);
    if (removed?.url) {
      URL.revokeObjectURL(removed.url);
    }
    if (mediaFiles.value.length === 0) {
      showVisionWarning.value = false;
    }
  };

  const clearMediaFiles = () => {
    mediaFiles.value.forEach((m) => {
      if (m.url) URL.revokeObjectURL(m.url);
    });
    mediaFiles.value = [];
    showVisionWarning.value = false;
  };

  const hideVisionWarning = () => {
    showVisionWarning.value = false;
  };

  return {
    mediaFiles,
    showVisionWarning,
    mediaTooltip,
    processDroppedFiles,
    handlePaste,
    removeMediaFile,
    clearMediaFiles,
    hideVisionWarning,
  };
}
