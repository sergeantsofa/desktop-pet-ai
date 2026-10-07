<script setup lang="ts">
/**
 * 自我升級進度條(浮在角色頭頂)。
 * 由 App.vue 在偵測到 dev_* 工具時驅動;chat-done 補到 100%、淡出。
 */
defineProps<{
  /** 目前階段的中文說明,如「讀自己的碼」「改自己的碼」 */
  label: string;
  /** 0~100 */
  progress: number;
  state: "working" | "done" | "failed";
}>();
</script>

<template>
  <div class="dev-progress" :class="state">
    <div class="dev-row">
      <span class="dev-icon">{{ state === "done" ? "✅" : state === "failed" ? "⚠️" : "🛠️" }}</span>
      <span class="dev-label">{{
        state === "done"
          ? "升級完成!"
          : state === "failed"
            ? "升級沒成功…"
            : `自我升級中… ${label}`
      }}</span>
      <span class="dev-pct">{{ Math.round(progress) }}%</span>
    </div>
    <div class="dev-track">
      <div class="dev-fill" :style="{ width: progress + '%' }"></div>
    </div>
  </div>
</template>

<style scoped>
.dev-progress {
  position: absolute;
  top: 6px;
  left: 50%;
  transform: translateX(-50%);
  min-width: 190px;
  max-width: 92%;
  background: rgba(38, 50, 78, 0.96);
  color: #fff;
  border-radius: 12px;
  padding: 7px 12px 9px;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.32);
  font-size: 12px;
  z-index: 50;
  animation: pop 0.18s ease-out;
}
.dev-row {
  display: flex;
  align-items: center;
  gap: 6px;
  white-space: nowrap;
}
.dev-icon {
  font-size: 13px;
}
.dev-label {
  flex: 1;
}
.dev-pct {
  font-variant-numeric: tabular-nums;
  opacity: 0.85;
}
.dev-track {
  margin-top: 6px;
  height: 6px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.18);
  overflow: hidden;
}
.dev-fill {
  height: 100%;
  border-radius: 4px;
  background: linear-gradient(90deg, #5b8def, #7aa7ff);
  transition: width 0.3s ease;
}
/* 進行中:流光,讓使用者感覺真的在動 */
.dev-progress.working .dev-fill {
  background-image: linear-gradient(90deg, #5b8def, #8fb4ff, #5b8def);
  background-size: 200% 100%;
  animation: shimmer 1.2s linear infinite;
}
.dev-progress.done .dev-fill {
  background: linear-gradient(90deg, #3ec97a, #5fe39a);
}
.dev-progress.failed .dev-fill {
  background: linear-gradient(90deg, #e36a6a, #ff8a8a);
}
@keyframes shimmer {
  from {
    background-position: 200% 0;
  }
  to {
    background-position: 0 0;
  }
}
@keyframes pop {
  from {
    opacity: 0;
    transform: translateX(-50%) translateY(-4px);
  }
  to {
    opacity: 1;
    transform: translateX(-50%) translateY(0);
  }
}
</style>
