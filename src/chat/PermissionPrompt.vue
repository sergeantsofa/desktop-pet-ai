<script setup lang="ts">
import type { PermissionRequest } from "../llm/api";

defineProps<{ request: PermissionRequest }>();
const emit = defineEmits<{ (e: "respond", allow: boolean): void }>();
</script>

<template>
  <div class="perm" @pointerdown.stop>
    <div class="perm-title">她想要:{{ request.label }}</div>
    <div class="perm-detail">{{ request.detail }}</div>
    <div class="perm-actions">
      <button class="allow" @click="emit('respond', true)">允許</button>
      <button class="deny" @click="emit('respond', false)">拒絕</button>
    </div>
  </div>
</template>

<style scoped>
.perm {
  position: absolute;
  left: 50%;
  bottom: 64px;
  transform: translateX(-50%);
  width: min(88%, 320px);
  background: rgba(255, 255, 255, 0.98);
  border-radius: 14px;
  padding: 12px 14px;
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.3);
  font-size: 13px;
  color: #333;
  animation: pop 0.18s ease-out;
}
.perm-title {
  font-weight: 600;
  margin-bottom: 4px;
}
.perm-detail {
  color: #666;
  font-size: 12px;
  word-break: break-all;
  max-height: 60px;
  overflow-y: auto;
  margin-bottom: 10px;
}
.perm-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}
button {
  border: none;
  border-radius: 14px;
  padding: 6px 16px;
  cursor: pointer;
  font-size: 13px;
}
.allow {
  background: #5b8def;
  color: #fff;
}
.allow:hover {
  background: #4a7de0;
}
.deny {
  background: #eee;
  color: #555;
}
.deny:hover {
  background: #ddd;
}
@keyframes pop {
  from {
    opacity: 0;
    transform: translateX(-50%) scale(0.92);
  }
  to {
    opacity: 1;
    transform: translateX(-50%) scale(1);
  }
}
</style>
