<script setup lang="ts">
import type { PhysicalKey } from "@mouse-stats/contracts";
defineProps<{ keyInfo: PhysicalKey; count: number; maximum: number }>();
</script>
<template>
  <div
    class="key"
    :style="{
      flex: keyInfo.width ?? 1,
      background: `rgba(96,211,181,${0.05 + (0.75 * count) / maximum})`,
    }"
    :title="`${keyInfo.label} / 0x${keyInfo.code.toString(16)}: ${count.toLocaleString()} 回`"
  >
    <span>{{ keyInfo.label }}</span
    ><small>{{ count.toLocaleString() }}</small>
  </div>
</template>
