<script setup lang="ts">
import { ref, watch, onUnmounted } from 'vue';

const props = defineProps<{ text: string; bottom?: number; variant?: string; fading?: boolean }>();

const displayed = ref('');
let timer: ReturnType<typeof setInterval> | null = null;
let index = 0;

function startTyping() {
  displayed.value = '';
  index = 0;
  if (timer) clearInterval(timer);
  timer = setInterval(() => {
    if (index < props.text.length) {
      displayed.value += props.text[index];
      index++;
    } else {
      if (timer) clearInterval(timer);
      timer = null;
    }
  }, 30);
}

watch(() => props.text, () => startTyping(), { immediate: true });

onUnmounted(() => {
  if (timer) clearInterval(timer);
});
</script>

<template>
  <!-- bottom 有給(角色頭頂上方)就用 bottom 定位,泡泡往上長;沒給則退回固定 top -->
  <div
    class="bubble"
    :class="['bubble--' + (props.variant || 'classic'), { fading: props.fading }]"
    :style="props.bottom != null ? { top: 'auto', bottom: props.bottom + 'px' } : undefined"
  >
    {{ displayed }}<span v-if="displayed.length < text.length" class="cursor">|</span>
    <div class="bubble-tail"></div>
  </div>
</template>

<style scoped>
.bubble {
  /* 每個樣式覆寫這些變數即可(預設 = 經典白) */
  --bubble-bg: rgba(255, 255, 255, 0.97);
  --bubble-color: #333;
  --bubble-border: none;
  --bubble-radius: 14px;
  --bubble-shadow: 0 4px 14px rgba(0, 0, 0, 0.25);
  --bubble-font: inherit;

  position: absolute;
  top: 14px;
  left: 50%;
  transform: translateX(-50%);
  max-width: 85%;
  background: var(--bubble-bg);
  color: var(--bubble-color);
  border: var(--bubble-border);
  border-radius: var(--bubble-radius);
  padding: 10px 14px;
  font-size: 16px;
  font-family: var(--bubble-font);
  line-height: 1.6;
  box-shadow: var(--bubble-shadow);
  animation: pop 0.18s ease-out;
}
.bubble-tail {
  position: absolute;
  bottom: -6px;
  left: 50%;
  transform: translateX(-50%) rotate(45deg);
  width: 12px;
  height: 12px;
  background: var(--bubble-bg);
  border: var(--bubble-border);
}

/* ---- 樣式變體 ---- */
.bubble--cloud {
  --bubble-radius: 26px;
  --bubble-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
}
.bubble--comic {
  --bubble-color: #111;
  --bubble-border: 3px solid #111;
  --bubble-radius: 16px;
  --bubble-shadow: 4px 4px 0 #111;
  font-weight: 700;
}
.bubble--neon {
  --bubble-bg: rgba(18, 16, 38, 0.95);
  --bubble-color: #7ef9ff;
  --bubble-border: 1.5px solid #7ef9ff;
  --bubble-radius: 14px;
  --bubble-shadow: 0 0 10px rgba(126, 249, 255, 0.65), 0 0 20px rgba(126, 249, 255, 0.35);
  animation: pop 0.18s ease-out, neonGlow 1.5s ease-in-out infinite alternate;
}
.bubble--pastel {
  --bubble-bg: linear-gradient(135deg, #ffd9ec, #d9e8ff);
  --bubble-color: #6a5a7a;
  --bubble-radius: 20px;
  --bubble-shadow: 0 6px 16px rgba(180, 150, 200, 0.4);
}
.bubble--pixel {
  --bubble-color: #222;
  --bubble-border: 3px solid #222;
  --bubble-radius: 0;
  --bubble-shadow: 4px 4px 0 rgba(0, 0, 0, 0.25);
  --bubble-font: "Courier New", monospace;
}
.bubble--candy {
  --bubble-bg: linear-gradient(135deg, #fff1c9, #ffd0a6);
  --bubble-color: #8a4b2b;
  --bubble-radius: 22px;
  --bubble-shadow: 0 6px 16px rgba(230, 160, 110, 0.45);
}
.bubble--ghost {
  --bubble-bg: rgba(255, 255, 255, 0.28);
  --bubble-color: #fff;
  --bubble-border: 1px solid rgba(255, 255, 255, 0.6);
  --bubble-radius: 16px;
  --bubble-shadow: 0 4px 18px rgba(0, 0, 0, 0.25);
  backdrop-filter: blur(6px);
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.55);
}
/* 退場:慢慢往上飄 + 淡出(保留水平置中的 translateX) */
.bubble.fading {
  animation: bubbleFloatUp 0.8s ease forwards;
}
@keyframes bubbleFloatUp {
  from {
    opacity: 1;
  }
  to {
    opacity: 0;
    transform: translateX(-50%) translateY(-34px);
  }
}
@keyframes neonGlow {
  from {
    box-shadow: 0 0 8px rgba(126, 249, 255, 0.5), 0 0 14px rgba(126, 249, 255, 0.25);
  }
  to {
    box-shadow: 0 0 14px rgba(126, 249, 255, 0.9), 0 0 28px rgba(126, 249, 255, 0.5);
  }
}
.cursor {
  animation: blink 0.6s step-end infinite;
  color: #888;
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
@keyframes blink {
  from, to { opacity: 1; }
  50% { opacity: 0; }
}
</style>
