<script setup lang="ts">
/**
 * 講話時飄起的小星星 / 愛心特效(需求第 6 點)。
 * 由 App.vue 用 v-if 控制顯示(她有泡泡=正在說話時掛上)。
 * pointer-events:none,純裝飾,不擋點擊。
 */
const EMOJIS = ["✨", "💕", "⭐", "💗", "✨", "🌟"];

// 元件掛上時隨機生成一組粒子(每次說話重新隨機)
const particles = Array.from({ length: 7 }, (_, i) => ({
  emoji: EMOJIS[Math.floor(Math.random() * EMOJIS.length)],
  left: Math.round(15 + Math.random() * 70), // %
  delay: (i * 0.3).toFixed(2),
  dur: (2.4 + Math.random() * 1.8).toFixed(2),
  size: Math.round(14 + Math.random() * 12),
}));
</script>

<template>
  <div class="sparkles">
    <span
      v-for="(p, i) in particles"
      :key="i"
      class="sp"
      :style="{
        left: p.left + '%',
        animationDelay: p.delay + 's',
        animationDuration: p.dur + 's',
        fontSize: p.size + 'px',
      }"
      >{{ p.emoji }}</span
    >
  </div>
</template>

<style scoped>
.sparkles {
  position: absolute;
  inset: 0;
  pointer-events: none;
  overflow: hidden;
  z-index: 4;
}
.sp {
  position: absolute;
  bottom: 30%;
  opacity: 0;
  animation-name: floatUp;
  animation-iteration-count: infinite;
  animation-timing-function: ease-out;
  will-change: transform, opacity;
}
@keyframes floatUp {
  0% {
    transform: translateY(0) scale(0.5) rotate(-8deg);
    opacity: 0;
  }
  20% {
    opacity: 0.95;
  }
  100% {
    transform: translateY(-130px) scale(1.05) rotate(8deg);
    opacity: 0;
  }
}
</style>
