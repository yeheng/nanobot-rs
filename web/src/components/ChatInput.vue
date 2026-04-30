<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { Send, Square } from 'lucide-vue-next';
import { computed, nextTick, ref } from 'vue';

const props = defineProps<{
  isConnected: boolean;
  sessionStatus: string;
  isThinking: boolean;
  isReceiving: boolean;
  waitingPhase?: string | null;
}>();

const emit = defineEmits<{
  (e: 'send', text: string): void;
  (e: 'stop'): void;
}>();

const inputRef = ref<HTMLTextAreaElement | null>(null);
const inputValue = ref('');

const inputPlaceholder = computed(() => {
  if (props.sessionStatus === 'receiving') return 'AI is processing...';
  if (props.sessionStatus === 'waiting_input') {
    return props.waitingPhase
      ? `💬 AI is waiting for your reply to continue...`
      : '💬 AI is waiting for your reply...';
  }
  return 'Type a message...';
});

const handleKeydown = (event: KeyboardEvent) => {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    submit();
  }
};

const autoResize = () => {
  const el = inputRef.value;
  if (!el) return;
  el.style.height = 'auto';
  el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
};

const submit = () => {
  const text = inputValue.value.trim();
  if (!text || !props.isConnected || props.sessionStatus === 'sending') return;
  emit('send', text);
  inputValue.value = '';
  nextTick(() => {
    inputRef.value?.focus();
    if (inputRef.value) inputRef.value.style.height = 'auto';
  });
};
</script>

<template>
  <div class="p-4 bg-transparent shrink-0">
    <div class="max-w-full md:max-w-4xl lg:max-w-5xl xl:max-w-6xl mx-auto w-full relative">
      <div class="flex items-end th-input-bg border th-border rounded-2xl p-2 shadow-xl backdrop-blur-xl transition-all"
        :class="{
          'focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/20': sessionStatus === 'idle' || sessionStatus === 'disconnected',
          'border-primary/30 ring-2 ring-primary/20': sessionStatus === 'receiving' || sessionStatus === 'sending'
        }">
        <textarea ref="inputRef" v-model="inputValue" @keydown="handleKeydown" @input="autoResize"
          :placeholder="inputPlaceholder"
          :disabled="!isConnected || sessionStatus === 'receiving' || sessionStatus === 'sending'"
          :class="{ 'border-amber-400/50 ring-2 ring-amber-400/20': sessionStatus === 'waiting_input' }"
          autofocus rows="1"
          class="flex-1 overflow-x-hidden border-0 bg-transparent shadow-none focus:outline-none focus:ring-0 th-text px-3 py-2.5 disabled:opacity-50 disabled:cursor-not-allowed resize-none custom-scrollbar min-h-[40px] max-h-[200px]"></textarea>

        <Button v-if="sessionStatus === 'receiving' || isThinking" @click="emit('stop')"
          class="w-9 h-9 rounded-xl bg-destructive/80 hover:bg-destructive text-white shrink-0 ml-2 transition-all" size="icon" title="Stop generating">
          <Square class="w-3.5 h-3.5 fill-current" />
        </Button>
        <Button v-else @click="submit" :disabled="!inputValue.trim() || !isConnected || sessionStatus === 'sending'"
          class="w-9 h-9 rounded-xl text-white shrink-0 ml-2 transition-all"
          :class="{ 'bg-primary hover:opacity-90': sessionStatus === 'idle' || sessionStatus === 'waiting_input', 'bg-muted cursor-not-allowed': sessionStatus !== 'idle' && sessionStatus !== 'waiting_input' }"
          size="icon">
          <Send class="w-4 h-4" />
        </Button>
      </div>
      <div class="flex items-center justify-center text-[10px] th-text-dim mt-2 px-1">
        <span>Shift+Enter for new line</span>
      </div>
    </div>
  </div>
</template>

<style>
.custom-scrollbar::-webkit-scrollbar { width: 6px; }
.custom-scrollbar::-webkit-scrollbar-track { background: rgba(0,0,0,0.05); border-radius: 4px; }
.dark .custom-scrollbar::-webkit-scrollbar-track { background: rgba(0,0,0,0.1); }
.custom-scrollbar::-webkit-scrollbar-thumb { background: rgba(0,0,0,0.2); border-radius: 4px; }
.dark .custom-scrollbar::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.2); }
.custom-scrollbar::-webkit-scrollbar-thumb:hover { background: rgba(0,0,0,0.3); }
.dark .custom-scrollbar::-webkit-scrollbar-thumb:hover { background: rgba(255,255,255,0.3); }
</style>
