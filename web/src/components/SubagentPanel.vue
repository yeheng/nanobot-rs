<script setup lang="ts">
import { ref, computed } from 'vue';
import type { SubagentState } from '../types';
import { ChevronDown, ChevronRight, Loader2, CheckCircle, XCircle, Wrench } from 'lucide-vue-next';

const props = defineProps<{
  subagents: Map<string, SubagentState>;
}>();

// 展开状态
const expandedIds = ref<Set<string>>(new Set());

// 按状态分组
const runningSubagents = computed(() =>
  [...props.subagents.values()]
    .filter(s => s.status === 'running')
    .sort((a, b) => a.index - b.index)
);

const completedSubagents = computed(() =>
  [...props.subagents.values()]
    .filter(s => s.status !== 'running')
    .sort((a, b) => a.index - b.index)
);

const hasAnySubagents = computed(() => props.subagents.size > 0);
const allCompleted = computed(() =>
  hasAnySubagents.value && runningSubagents.value.length === 0
);

const toggleExpand = (id: string) => {
  if (expandedIds.value.has(id)) {
    expandedIds.value.delete(id);
  } else {
    expandedIds.value.add(id);
  }
};

const formatDuration = (start: number, end?: number) => {
  const ms = (end || Date.now()) - start;
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60000)}m ${Math.floor((ms % 60000) / 1000)}s`;
};

const statusIcon = (status: SubagentState['status']) => {
  switch (status) {
    case 'running': return Loader2;
    case 'completed': return CheckCircle;
    case 'error': return XCircle;
  }
};

const statusColor = (status: SubagentState['status']) => {
  switch (status) {
    case 'running': return 'text-primary';
    case 'completed': return 'text-primary';
    case 'error': return 'text-destructive';
  }
};

const statusBg = (status: SubagentState['status']) => {
  switch (status) {
    case 'running': return 'border-primary/30 bg-primary/5';
    case 'completed': return 'border-primary/30 bg-primary/5';
    case 'error': return 'border-destructive/30 bg-destructive/5';
  }
};
</script>

<template>
  <div v-if="hasAnySubagents" class="subagent-panel my-4">
    <!-- 运行中的 subagent 列表 -->
    <div v-if="runningSubagents.length > 0" class="space-y-2">
      <div class="flex items-center gap-2 text-sm th-text-muted mb-3">
        <Loader2 class="w-4 h-4 animate-spin" />
        <span class="font-medium">并行任务 ({{ runningSubagents.length }}个)</span>
      </div>

      <div
        v-for="subagent in runningSubagents"
        :key="subagent.id"
        class="subagent-card border rounded-lg overflow-hidden transition-all"
        :class="statusBg(subagent.status)"
      >
        <div
          class="subagent-header flex items-center gap-2 p-3 cursor-pointer th-hover transition-colors"
          @click="toggleExpand(subagent.id)"
        >
          <component
            :is="statusIcon(subagent.status)"
            class="w-4 h-4 shrink-0"
            :class="[statusColor(subagent.status), { 'animate-spin': subagent.status === 'running' }]"
          />
          <span class="font-medium th-text-secondary text-sm">Task {{ subagent.index }}</span>
          <span class="th-text-muted text-sm truncate flex-1">{{ subagent.task }}</span>
          <span class="text-xs th-text-dim flex items-center gap-1 shrink-0">
            <Wrench class="w-3 h-3" />
            {{ subagent.toolCount }}
          </span>
          <span class="text-xs th-text-dim shrink-0">{{ formatDuration(subagent.startTime) }}</span>
          <component
            :is="expandedIds.has(subagent.id) ? ChevronDown : ChevronRight"
            class="w-4 h-4 th-text-dim shrink-0"
          />
        </div>

        <!-- 展开详情 -->
        <Transition
          enter-active-class="transition-all duration-200 ease-out"
          leave-active-class="transition-all duration-150 ease-in"
          enter-from-class="opacity-0 max-h-0"
          leave-to-class="opacity-0 max-h-0"
        >
          <div v-if="expandedIds.has(subagent.id)" class="subagent-details border-t th-border">
            <div class="p-3 pl-6 space-y-2 text-sm">
              <!-- 思考过程 -->
              <div v-if="subagent.thinking" class="text-xs">
                <span class="th-text-muted font-medium">思考：</span>
                <p class="th-text-secondary mt-1 whitespace-pre-wrap break-words">{{ subagent.thinking.slice(0, 300) }}{{ subagent.thinking.length > 300 ? '...' : '' }}</p>
              </div>
              <!-- 输出内容 -->
              <div v-if="subagent.content" class="text-xs">
                <span class="th-text-muted font-medium">输出：</span>
                <p class="th-text mt-1 whitespace-pre-wrap break-words">{{ subagent.content.slice(0, 500) }}{{ subagent.content.length > 500 ? '...' : '' }}</p>
              </div>
              <!-- 工具调用 -->
              <div v-if="subagent.toolCalls.length > 0" class="text-xs">
                <span class="th-text-muted font-medium">工具调用：</span>
                <div class="mt-1 space-y-1">
                  <div
                    v-for="tool in subagent.toolCalls.slice(0, 5)"
                    :key="tool.id"
                    class="flex items-center gap-2 th-text-secondary"
                  >
                    <component
                      :is="tool.status === 'running' ? Loader2 : (tool.status === 'error' ? XCircle : CheckCircle)"
                      class="w-3 h-3 shrink-0"
                      :class="{ 'animate-spin': tool.status === 'running' }"
                    />
                    <span class="truncate">{{ tool.name }}</span>
                    <span v-if="tool.duration" class="th-text-dim text-xs">({{ tool.duration }})</span>
                  </div>
                  <div v-if="subagent.toolCalls.length > 5" class="th-text-dim pl-5">
                    ... 还有 {{ subagent.toolCalls.length - 5 }} 个
                  </div>
                </div>
              </div>
            </div>
          </div>
        </Transition>
      </div>
    </div>

    <!-- 已完成的摘要 -->
    <div v-if="allCompleted && completedSubagents.length > 0" class="completed-summary mt-4 p-4 bg-primary/5 border border-primary/20 rounded-lg">
      <div class="flex items-center gap-2 text-primary mb-3">
        <CheckCircle class="w-4 h-4" />
        <span class="font-medium text-sm">完成 {{ completedSubagents.length }} 个并行任务</span>
      </div>

      <div class="space-y-1.5">
        <div
          v-for="s in completedSubagents"
          :key="s.id"
          class="flex items-center gap-2 text-sm py-1.5 px-2 rounded bg-muted"
        >
          <component
            :is="statusIcon(s.status)"
            class="w-3.5 h-3.5 shrink-0"
            :class="statusColor(s.status)"
          />
          <span class="th-text-muted w-14 shrink-0">Task {{ s.index }}:</span>
          <span class="th-text-secondary truncate flex-1">{{ s.summary || s.task }}</span>
          <Wrench class="w-3 h-3 th-text-dim shrink-0" />
          <span class="text-xs th-text-dim w-6 text-right shrink-0">{{ s.toolCount }}</span>
          <span class="text-xs th-text-dim w-12 text-right shrink-0">{{ formatDuration(s.startTime, s.endTime) }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.subagent-panel {
  animation: fadeIn 0.2s ease-out;
}

@keyframes fadeIn {
  from {
    opacity: 0;
    transform: translateY(-4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.subagent-details {
  max-height: 300px;
  overflow-y: auto;
}
</style>
