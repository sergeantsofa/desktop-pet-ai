<script setup lang="ts">
import { onMounted, ref } from "vue";

const emit = defineEmits<{
  (e: "submit", text: string): void;
  (e: "close"): void;
}>();

const inputRef = ref<HTMLInputElement | null>(null);
const text = ref("");

function submit(): void {
  const t = text.value.trim();
  if (!t) return;
  emit("submit", t);
  text.value = "";
}

onMounted(() => inputRef.value?.focus());
</script>

<template>
  <div class="chat-input-wrap">
    <input
      ref="inputRef"
      v-model="text"
      class="chat-input"
      type="text"
      placeholder="跟我說點什麼…(Enter 送出,Esc 關閉)"
      @keydown.enter="submit"
      @keydown.esc="emit('close')"
      @pointerdown.stop
    />
  </div>
</template>

<style scoped>
.chat-input-wrap {
  position: absolute;
  left: 50%;
  bottom: 16px;
  transform: translateX(-50%);
  width: min(90%, 360px);
}
.chat-input {
  width: 100%;
  box-sizing: border-box;
  padding: 10px 14px;
  border: none;
  border-radius: 20px;
  outline: none;
  font-size: 14px;
  background: rgba(255, 255, 255, 0.96);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
}
</style>
