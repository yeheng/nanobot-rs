<script setup lang="ts">
import { computed, ref } from 'vue';
import { ChevronDown } from 'lucide-vue-next';
import type { Message } from '../types';

const props = defineProps<{
  message: Message;
  isThinking: boolean;
  isLastBotMessage: boolean;
}>();

const thinkingOpen = ref(false);
const toolsOpen = ref(false);

const isThinkingRunning = computed(() => props.isLastBotMessage && props.isThinking && !!props.message.thinking);
const runningToolCount = computed(() => props.message.toolCalls?.filter(t => t.status === 'running').length || 0);
const completedToolCount = computed(() => props.message.toolCalls?.filter(t => t.status === 'complete').length || 0);
const totalToolCount = computed(() => props.message.toolCalls?.length || 0);
const hasToolErrors = computed(() => props.message.toolCalls?.some(t => t.status === 'error') || false);
</script>

<template>
  <div class="flex flex-wrap items-center gap-1.5 my-0.5">
    <!-- Thinking badge -->
    <button v-if="message.thinking" @click="thinkingOpen = !thinkingOpen"
      class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[10px] font-medium transition-colors"
      :class="thinkingOpen ? 'bg-violet-500/20 text-violet-200' : 'bg-violet-500/10 text-violet-300 hover:bg-violet-500/20'">
      <span class="relative flex h-1.5 w-1.5">
        <span v-if="isThinkingRunning" class="animate-ping absolute inline-flex h-full w-full rounded-full bg-violet-400 opacity-75"></span>
        <span class="relative inline-flex rounded-full h-1.5 w-1.5" :class="isThinkingRunning ? 'bg-violet-400' : 'bg-violet-500'"></span>
      </span>
      <span>Thinking</span>
      <ChevronDown class="w-3 h-3 transition-transform" :class="{ 'rotate-180': thinkingOpen }" />
    </button>

    <!-- Tool Calls badge -->
    <button v-if="message.toolCalls && message.toolCalls.length > 0" @click="toolsOpen = !toolsOpen"
      class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[10px] font-medium transition-colors"
      :class="toolsOpen ? 'bg-amber-500/20 text-amber-200' : 'bg-amber-500/10 text-amber-300 hover:bg-amber-500/20'">
      <span class="relative flex h-1.5 w-1.5">
        <span v-if="runningToolCount > 0" class="animate-ping absolute inline-flex h-full w-full rounded-full bg-amber-400 opacity-75"></span>
        <span class="relative inline-flex rounded-full h-1.5 w-1.5"
          :class="runningToolCount > 0 ? 'bg-amber-400' : hasToolErrors ? 'bg-red-400' : 'bg-emerald-400'"></span>
      </span>
      <span>Tools {{ completedToolCount }}/{{ totalToolCount }}</span>
      <ChevronDown class="w-3 h-3 transition-transform" :class="{ 'rotate-180': toolsOpen }" />
    </button>
  </div>

  <!-- Thinking content -->
  <div v-show="thinkingOpen && message.thinking" class="px-3 py-2 rounded-xl bg-violet-500/10 border border-violet-500/20 text-slate-300 text-xs whitespace-pre-wrap">
    {{ message.thinking }}
  </div>

  <!-- Tool Calls content -->
  <div v-show="toolsOpen && message.toolCalls && message.toolCalls.length > 0" class="flex flex-col gap-1.5">
    <div v-for="tool in message.toolCalls" :key="tool.id"
      class="px-3 py-2 rounded-xl bg-amber-500/10 border border-amber-500/20 text-xs">
      <div class="flex items-center justify-between mb-1">
        <span class="font-medium text-amber-200">{{ tool.name }}</span>
        <span v-if="tool.status === 'running'" class="text-[10px] text-amber-300">Running...</span>
        <span v-else-if="tool.status === 'complete'" class="text-[10px] text-emerald-400">Done {{ tool.duration ? `(${tool.duration})` : '' }}</span>
        <span v-else class="text-[10px] text-red-400">Error</span>
      </div>
      <div v-if="tool.arguments" class="text-slate-400 text-[10px] font-mono truncate">Args: {{ tool.arguments }}</div>
      <div v-if="tool.result" class="text-slate-300 text-[10px] mt-1 border-t border-amber-500/10 pt-1">{{ tool.result }}</div>
    </div>
  </div>
</template>
