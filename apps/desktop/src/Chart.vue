<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, watch } from "vue";
import * as echarts from "echarts/core";
import { BarChart } from "echarts/charts";
import { GridComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
echarts.use([BarChart, GridComponent, TooltipComponent, CanvasRenderer]);
const props = defineProps<{ labels: string[]; values: number[]; unit: string }>();
const el = ref<HTMLDivElement>();
let chart: echarts.ECharts | undefined;
let observer: ResizeObserver;
let drawnSignature: string | undefined;
function draw() {
  const signature = JSON.stringify([props.labels, props.values, props.unit]);
  if (!chart || signature === drawnSignature) return;
  drawnSignature = signature;
  chart?.setOption({
    backgroundColor: "transparent",
    grid: { left: 65, right: 20, top: 20, bottom: 70 },
    tooltip: {
      trigger: "axis",
      valueFormatter: (v: number) => `${v.toLocaleString()} ${props.unit}`,
    },
    xAxis: {
      type: "category",
      data: props.labels,
      axisLabel: { color: "#8b9eaf", rotate: 35, fontSize: 10 },
      axisLine: { lineStyle: { color: "#293844" } },
    },
    yAxis: {
      type: "value",
      axisLabel: { color: "#8b9eaf" },
      splitLine: { lineStyle: { color: "#22303b" } },
    },
    series: [
      {
        type: "bar",
        data: props.values,
        itemStyle: { color: "#60d3b5", borderRadius: [3, 3, 0, 0] },
        barMaxWidth: 28,
      },
    ],
  });
}
onMounted(() => {
  chart = echarts.init(el.value!);
  draw();
  observer = new ResizeObserver(() => chart?.resize());
  observer.observe(el.value!);
});
watch(() => [props.labels, props.values, props.unit], draw, { deep: true });
onBeforeUnmount(() => {
  observer?.disconnect();
  chart?.dispose();
});
</script>
<template>
  <div ref="el" class="chart" role="img" :aria-label="`${unit}の時間別統計`"></div>
</template>
