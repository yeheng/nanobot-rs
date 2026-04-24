<script setup lang="ts">
import { computed, ref } from 'vue';
import { Sparkles, ChevronDown, Loader2, CheckCircle, XCircle, Wrench } from 'lucide-vue-next';
import type { Message, TimelineItem } from '../types';

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

/** Build a chronological timeline of thinking chunks and tool calls.
 *  Adjacent thinking chunks are merged so that streaming tokens don't
 *  render as separate boxed fragments.
 */
const timeline = computed((): TimelineItem[] => {
  const rawItems: TimelineItem[] = [];

  // Add thinking chunks
  const chunks = props.message.thinkingChunks || [];
  for (const chunk of chunks) {
    rawItems.push({ type: 'thinking', content: chunk.content, timestamp: chunk.timestamp });
  }

  // Add tool calls (using their start time)
  const tools = props.message.toolCalls || [];
  for (const tool of tools) {
    rawItems.push({ type: 'tool_call', tool, timestamp: tool.startTime || 0 });
  }

  // Sort by timestamp ascending
  rawItems.sort((a, b) => a.timestamp - b.timestamp);

  // Merge adjacent thinking items to avoid fragmented UI
  const items: TimelineItem[] = [];
  for (const item of rawItems) {
    if (item.type === 'thinking') {
      const last = items[items.length - 1];
      if (last && last.type === 'thinking') {
        last.content += item.content;
      } else {
        items.push({ ...item });
      }
    } else {
      items.push({ ...item });
    }
  }

  return items;
});

/** Hard limit for tool result display to avoid UI clutter. */
const TOOL_RESULT_MAX_CHARS = 200;

const truncateResult = (text: string): string => {
  const chars = Array.from(text);
  if (chars.length <= TOOL_RESULT_MAX_CHARS) return text;
  return chars.slice(0, TOOL_RESULT_MAX_CHARS).join('') + '...';
};
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

    <!-- Expanded content: chronological timeline -->
    <div
      v-show="expanded"
      class="mt-1 px-3 py-2 rounded-xl border th-surface th-border text-xs space-y-2"
    >
      <template v-for="(item, idx) in timeline" :key="idx">
        <!-- Thinking chunk -->
        <div v-if="item.type === 'thinking'" class="flex gap-2">
          <Sparkles class="w-3 h-3 text-primary shrink-0 mt-0.5" />
          <div class="th-text-secondary whitespace-pre-wrap leading-relaxed flex-1">
            {{ item.content }}
          </div>
        </div>

        <!-- Tool call -->
        <div v-else-if="item.type === 'tool_call'" class="flex gap-2 p-1.5 rounded-lg th-active-bg">
          <component
            :is="item.tool.status === 'running' ? Loader2 : (item.tool.status === 'error' ? XCircle : CheckCircle)"
            class="w-3.5 h-3.5 shrink-0 mt-0.5"
            :class="{
              'text-primary animate-spin': item.tool.status === 'running',
              'text-primary': item.tool.status === 'complete',
              'text-destructive': item.tool.status === 'error'
            }"
          />
          <div class="flex-1 min-w-0">
            <div class="flex items-center justify-between gap-2">
              <span class="font-medium th-text-secondary truncate">{{ item.tool.name }}</span>
              <span
                v-if="item.tool.duration"
                class="text-[10px] th-text-dim shrink-0"
              >
                {{ item.tool.duration }}
              </span>
            </div>
            <div
              v-if="item.tool.arguments"
              class="text-[10px] th-text-muted font-mono truncate mt-0.5"
            >
              {{ item.tool.arguments }}
            </div>

            <!-- Tool result — hard-truncated to 200 chars -->
            <div
              v-if="item.tool.result"
              class="text-[10px] th-text-secondary mt-1 whitespace-pre-wrap break-words leading-relaxed bg-muted/30 rounded p-1.5"
            >
              {{ truncateResult(item.tool.result) }}
            </div>
          </div>
        </div>
      </template>

      <!-- Fallback: if no timeline but has old-format thinking -->
      <div v-if="timeline.length === 0 && hasThinking" class="flex gap-2">
        <Sparkles class="w-3 h-3 text-primary shrink-0 mt-0.5" />
        <div class="th-text-secondary whitespace-pre-wrap leading-relaxed flex-1">
          {{ message.thinking }}
        </div>
      </div>
    </div>
  </div>
</template>
