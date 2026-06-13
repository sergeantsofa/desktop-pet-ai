<script setup lang="ts">
defineProps<{ text: string }>();
</script>

<template>
  <div class="bubble">
    {{ text }}
    <div class="bubble-tail"></div>
  </div>
</template>

<style scoped>
.bubble {
  position: absolute;
  top: 14px;
  left: 50%;
  transform: translateX(-50%);
  max-width: 85%;
  background: rgba(255, 255, 255, 0.97);
  color: #333;
  border-radius: 14px;
  padding: 10px 14px;
  font-size: 13px;
  line-height: 1.6;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.25);
  animation: pop 0.18s ease-out;
}
.bubble-tail {
  position: absolute;
  bottom: -6px;
  left: 50%;
  transform: translateX(-50%) rotate(45deg);
  width: 12px;
  height: 12px;
  background: rgba(255, 255, 255, 0.97);
}
@keyframes pop {
  from {
    opacity: 0;
    transform: translateX(-50%) scale(0.9);
  }
  to {
    opacity: 1;
    transform: translateX(-50%) scale(1);
  }
}
</style>
