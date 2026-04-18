<script setup lang="ts">
import { computed, ref } from 'vue';
import { Sparkles, ChevronDown, Loader2, CheckCircle, XCircle, Wrench } from 'lucide-vue-next';
import type { Message } from '../types';

const props = defineProps<{
  message: Message;
  isThinking: boolean;
  isLastBotMessage: boolean;
}>();

const expanded = ref(false);

const hasThinking = computed(() => !!props.message.thinking);
const hasTools = computed(() => (props.message.toolCalls?.length || 0) > 0);
const runningToolCount = computed(() => props.message.toolCalls?.filter(t => t.status === 'running').length || 0);
const completedToolCount = computed(() => props.message.toolCalls?.filter(t => t.status === 'complete').length || 0);
const totalToolCount = computed(() => props.message.toolCalls?.length || 0);
const isActive = computed(() => props.isLastBotMessage && (props.isThinking || runningToolCount.value > 0));
</script>

<template>
  <div v-if="hasThinking || hasTools" class="w-full my-1">
    <!-- Collapsible header -->
    <button
      @click="expanded = !expanded"
      class="w-full flex items-center justify-between gap-2 px-3 py-2 rounded-xl border transition-all"
      :class="[
        isActive
          ? 'bg-primary/5 border-primary/20'
          : 'th-surface-raised th-border th-hover'
      ]"
    >
      <div class="flex items-center gap-2">
        <span class="relative flex h-2 w-2">
          <span
            v-if="isActive"
            class="animate-ping absolute inline-flex h-full w-full rounded-full bg-primary opacity-75"
          />
          <span
            class="relative inline-flex rounded-full h-2 w-2"
            :class="isActive ? 'bg-primary' : 'bg-muted-foreground'"
          />
        </span>
        <Sparkles class="w-3.5 h-3.5 text-primary" />
        <span class="text-[11px] font-medium th-text-secondary">
          Thoughts
        </span>
        <span
          v-if="hasTools"
          class="text-[10px] th-text-muted flex items-center gap-1"
        >
          <Wrench class="w-3 h-3" />
          {{ completedToolCount }}/{{ totalToolCount }}
        </span>
      </div>

      <div class="flex items-center gap-1.5 text-[10px] th-text-muted">
        <span v-if="!expanded" class="hidden sm:inline">Expand to view model thoughts</span>
        <ChevronDown
          class="w-4 h-4 text-muted-foreground transition-transform"
          :class="{ 'rotate-180': expanded }"
        />
      </div>
    </button>

    <!-- Expanded content -->
    <div
      v-show="expanded"
      class="mt-1 px-3 py-2 rounded-xl border th-surface th-border text-xs space-y-2"
    >
      <!-- Thinking content -->
      <div v-if="hasThinking">
        <div class="text-[10px] font-medium th-text-muted mb-1 flex items-center gap-1">
          <Sparkles class="w-3 h-3" /> Reasoning
        </div>
        <div class="th-text-secondary whitespace-pre-wrap leading-relaxed">
          {{ message.thinking }}
        </div>
      </div>

      <!-- Tool calls -->
      <div v-if="hasTools" :class="{ 'pt-2 border-t border-border': hasThinking }">
        <div class="text-[10px] font-medium th-text-muted mb-1 flex items-center gap-1">
          <Wrench class="w-3 h-3" /> Tool Calls
        </div>
        <div class="space-y-1.5">
          <div
            v-for="tool in message.toolCalls"
            :key="tool.id"
            class="flex items-start gap-2 p-1.5 rounded-lg th-active-bg"
          >
            <component
              :is="tool.status === 'running' ? Loader2 : (tool.status === 'error' ? XCircle : CheckCircle)"
              class="w-3.5 h-3.5 shrink-0 mt-0.5"
              :class="{
                'text-primary animate-spin': tool.status === 'running',
                'text-primary': tool.status === 'complete',
                'text-destructive': tool.status === 'error'
              }"
            />
            <div class="flex-1 min-w-0">
              <div class="flex items-center justify-between gap-2">
                <span class="font-medium th-text-secondary truncate">{{ tool.name }}</span>
                <span
                  v-if="tool.duration"
                  class="text-[10px] th-text-dim shrink-0"
                >
                  {{ tool.duration }}
                </span>
              </div>
              <div
                v-if="tool.arguments"
                class="text-[10px] th-text-muted font-mono truncate mt-0.5"
              >
                {{ tool.arguments }}
              </div>
              <div
                v-if="tool.result"
                class="text-[10px] th-text-secondary mt-1"
              >
                {{ tool.result }}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
