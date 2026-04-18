<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Menu as HeadlessMenu, MenuButton, MenuItem, MenuItems } from '@headlessui/vue';
import { AlertCircle, ArrowDown, Bot, Check, Cpu, Loader2, Moon, MoreVertical, Palette, RotateCcw, Send, Sparkles, Square, Sun, Trash2, X as XIcon } from 'lucide-vue-next';
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { useTheme, type ThemeHue } from '../composables/useTheme';
import { useChatSession } from '../composables/useChatSession';
import { useChatStore } from '../stores/chatStore';
import ChatTimeDivider from './ChatTimeDivider.vue';
import MessageBubble from './MessageBubble.vue';
import SubagentPanel from './SubagentPanel.vue';

const props = defineProps<{ chatId: string }>();
const emit = defineEmits<{ (e: 'connection-status', status: boolean): void }>();

const chatStore = useChatStore();
const { mode, hue, setMode, setHue, hues } = useTheme();

const hueMeta: Record<ThemeHue, { label: string; dot: string }> = {
  zinc:    { label: 'Zinc',    dot: 'bg-zinc-500' },
  blue:    { label: 'Blue',    dot: 'bg-blue-500' },
  rose:    { label: 'Rose',    dot: 'bg-rose-500' },
  emerald: { label: 'Emerald', dot: 'bg-emerald-500' },
  amber:   { label: 'Amber',   dot: 'bg-amber-500' },
  violet:  { label: 'Violet',  dot: 'bg-violet-500' },
};

const session = useChatSession(computed(() => props.chatId));

watch(() => session.isConnected, (val) => {
  emit('connection-status', val);
}, { immediate: true });

// Scroll
const scrollAreaRef = ref<InstanceType<typeof ScrollArea> | null>(null);
const showScrollButton = ref(false);
const userScrolledUp = ref(false);

const getScrollElement = (scrollArea: InstanceType<typeof ScrollArea> | null): HTMLElement | null => {
  if (!scrollArea) return null;
  const el = scrollArea.$el as HTMLElement;
  if (!el) return null;
  const viewport = el.querySelector('[data-radix-scroll-area-viewport]') as HTMLElement;
  return viewport || el;
};

const scrollToBottom = async (force = false) => {
  await nextTick();
  const scrollEl = getScrollElement(scrollAreaRef.value);
  if (!scrollEl) return;
  if (!force && userScrolledUp.value) return;
  scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: force ? 'smooth' : 'auto' });
};

const forceScrollToBottom = () => {
  userScrolledUp.value = false;
  scrollToBottom(true);
};

let scrollListener: (() => void) | null = null;

const setupScrollObserver = () => {
  nextTick(() => {
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (!scrollEl) return;
    const onScroll = () => {
      const distFromBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight;
      const nearBottom = distFromBottom < 100;
      showScrollButton.value = distFromBottom > 200;
      userScrolledUp.value = !nearBottom;
    };
    scrollEl.addEventListener('scroll', onScroll);
    scrollListener = () => scrollEl.removeEventListener('scroll', onScroll);
  });
};

onUnmounted(() => {
  scrollListener?.();
});

// Messages
const messages = computed(() => chatStore.activeMessages);
const hasUserMessages = computed(() => messages.value.some(m => m.role === 'user' || m.role === 'bot'));

watch(() => messages.value.length, () => scrollToBottom());
watch(() => props.chatId, () => {
  session.connect();
  session.fetchContext();
  userScrolledUp.value = false;
  nextTick(() => scrollToBottom(true));
});

onMounted(() => {
  session.connect();
  session.fetchContext();
  nextTick(() => scrollToBottom(true));
  setupScrollObserver();
});

// Input
const inputRef = ref<HTMLTextAreaElement | null>(null);
const inputValue = ref('');

const suggestedPrompts = [
  { icon: '💡', text: 'Explain how this project is structured' },
  { icon: '🔍', text: 'Help me find and fix bugs in my code' },
  { icon: '📝', text: 'Write a unit test for a function' },
  { icon: '🚀', text: 'Suggest performance improvements' },
];

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
  const ok = session.sendMessage(inputValue.value);
  if (ok) {
    inputValue.value = '';
    nextTick(() => {
      inputRef.value?.focus();
      if (inputRef.value) inputRef.value.style.height = 'auto';
    });
  }
};

const sendPrompt = (prompt: string) => {
  inputValue.value = prompt;
  submit();
};

const retryMessage = (msgId: string, content: string) => {
  session.retryMessage(msgId, content);
};

const clearHistory = () => {
  chatStore.clearChatMessages(props.chatId);
};
</script>

<template>
  <div class="flex h-full w-full relative">
    <div class="flex flex-col flex-1 min-w-0">
      <!-- Header -->
      <header class="py-3 px-5 th-header-bg border-b th-border flex justify-between items-center shrink-0">
        <div class="flex items-center gap-3">
          <div class="w-9 h-9 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center">
            <Bot class="w-5 h-5 text-white" />
          </div>
          <div>
            <div class="text-sm font-semibold th-text">Model</div>
            <div class="text-[10px] th-text-muted flex items-center gap-1.5">
              <span class="w-1.5 h-1.5 rounded-full" :class="session.isConnected ? 'bg-primary' : 'bg-destructive'" />
              {{ session.isConnected ? 'Online' : 'Offline' }}
              <span class="th-text-dim">|</span>
              <span
                class="flex items-center gap-1"
                :class="{
                  'text-destructive': session.sessionStatus === 'disconnected',
                  'text-primary': session.sessionStatus === 'sending' || session.sessionStatus === 'receiving',
                  'th-text-dim': session.sessionStatus === 'idle'
                }"
              >
                <Loader2 v-if="session.sessionStatus === 'sending' || session.sessionStatus === 'receiving'" class="w-3 h-3 animate-spin" />
                <span v-if="session.sessionStatus === 'disconnected'">Disconnected</span>
                <span v-else-if="session.sessionStatus === 'sending'">Sending...</span>
                <span v-else-if="session.sessionStatus === 'receiving'">Thinking...</span>
                <span v-else>Ready</span>
              </span>
            </div>
          </div>
        </div>

        <div class="flex items-center gap-2">
          <!-- Context stats inline -->
          <div v-if="session.contextStats" class="hidden md:flex items-center gap-2 mr-1">
            <div class="text-[10px] th-text-secondary font-medium whitespace-nowrap">
              Context: {{ session.contextStats.usage_percent.toFixed(1) }}%
            </div>
            <div class="w-20 lg:w-28 h-1.5 bg-muted rounded-full overflow-hidden">
              <div class="h-full rounded-full transition-all duration-500" :class="session.usageColor" :style="{ width: Math.min(session.contextStats.usage_percent, 100) + '%' }" />
            </div>
            <div v-if="session.watermarkInfo" class="hidden lg:block text-[10px] th-text-muted whitespace-nowrap">
              {{ session.watermarkInfo.watermark }}/{{ session.watermarkInfo.max_sequence }}
            </div>
            <Button variant="outline" size="sm" class="h-6 text-[10px] px-2 th-surface th-border th-hover th-text-secondary"
              :disabled="session.isCompacting" @click="session.forceCompact">
              <Cpu v-if="!session.isCompacting" class="w-3 h-3 mr-1" />
              <Loader2 v-else class="w-3 h-3 mr-1 animate-spin" />
              {{ session.isCompacting ? '...' : 'Compress' }}
            </Button>
          </div>

          <Button v-if="session.showReconnectButton" variant="outline" size="sm" @click="session.manualReconnect"
            class="text-primary border-primary/30 hover:bg-primary/10 text-xs h-8">
            <RotateCcw class="w-3.5 h-3.5 mr-1.5" />
            Reconnect
          </Button>

          <HeadlessMenu as="div" class="relative">
            <MenuButton as="button" class="p-2 rounded-md th-hover th-text-muted hover:th-text transition-colors">
              <MoreVertical class="w-4 h-4" />
            </MenuButton>
            <transition
              enter-active-class="transition duration-100 ease-out"
              enter-from-class="transform scale-95 opacity-0"
              enter-to-class="transform scale-100 opacity-100"
              leave-active-class="transition duration-75 ease-in"
              leave-from-class="transform scale-100 opacity-100"
              leave-to-class="transform scale-95 opacity-0"
            >
              <MenuItems class="absolute right-0 top-10 z-30 w-40 origin-top-right rounded-lg bg-popover border border-border shadow-lg focus:outline-none py-1">
                <MenuItem v-slot="{ active }">
                  <button @click="clearHistory" :class="[active ? 'bg-accent' : '', 'group flex w-full items-center px-3 py-2 text-xs th-text-secondary']">
                    <Trash2 class="w-3.5 h-3.5 mr-2 th-text-dim" />
                    Clear History
                  </button>
                </MenuItem>
              </MenuItems>
            </transition>
          </HeadlessMenu>

          <HeadlessMenu as="div" class="relative">
            <MenuButton as="button" class="p-2 rounded-md th-hover th-text-muted hover:th-text transition-colors">
              <Palette class="w-4 h-4" />
            </MenuButton>
            <transition
              enter-active-class="transition duration-100 ease-out"
              enter-from-class="transform scale-95 opacity-0"
              enter-to-class="transform scale-100 opacity-100"
              leave-active-class="transition duration-75 ease-in"
              leave-from-class="transform scale-100 opacity-100"
              leave-to-class="transform scale-95 opacity-0"
            >
              <MenuItems class="absolute right-0 top-10 z-30 w-44 origin-top-right rounded-lg bg-popover border border-border shadow-lg focus:outline-none py-1">
                <!-- Mode -->
                <div class="px-3 py-1.5 text-[10px] font-semibold th-text-muted uppercase tracking-wider">Mode</div>
                <MenuItem v-slot="{ active }">
                  <button
                    @click="setMode('light')"
                    :class="[active ? 'bg-accent' : '', 'group flex w-full items-center px-3 py-2 text-xs th-text-secondary']"
                  >
                    <Sun class="w-3.5 h-3.5 mr-2 th-text-dim" />
                    <span class="flex-1 text-left">Light</span>
                    <Check v-if="mode === 'light'" class="w-3 h-3 th-text-muted shrink-0" />
                  </button>
                </MenuItem>
                <MenuItem v-slot="{ active }">
                  <button
                    @click="setMode('dark')"
                    :class="[active ? 'bg-accent' : '', 'group flex w-full items-center px-3 py-2 text-xs th-text-secondary']"
                  >
                    <Moon class="w-3.5 h-3.5 mr-2 th-text-dim" />
                    <span class="flex-1 text-left">Dark</span>
                    <Check v-if="mode === 'dark'" class="w-3 h-3 th-text-muted shrink-0" />
                  </button>
                </MenuItem>
                <div class="my-1 border-t border-border" />
                <!-- Hue -->
                <div class="px-3 py-1.5 text-[10px] font-semibold th-text-muted uppercase tracking-wider">Hue</div>
                <MenuItem v-for="h in hues" :key="h" v-slot="{ active }">
                  <button
                    @click="setHue(h)"
                    :class="[active ? 'bg-accent' : '', 'group flex w-full items-center px-3 py-2 text-xs th-text-secondary']"
                  >
                    <span class="w-3 h-3 rounded-full mr-2 shrink-0" :class="hueMeta[h].dot" />
                    <span class="flex-1 text-left">{{ hueMeta[h].label }}</span>
                    <Check v-if="hue === h" class="w-3 h-3 th-text-muted shrink-0" />
                  </button>
                </MenuItem>
              </MenuItems>
            </transition>
          </HeadlessMenu>
        </div>
      </header>

      <!-- Error Banner -->
      <div v-if="session.errorBanner"
        class="mx-4 mt-2 flex items-center gap-2 bg-destructive/15 border border-destructive/30 text-destructive px-3 py-2 rounded-lg text-xs animate-in fade-in slide-in-from-top-2 duration-300 shrink-0">
        <AlertCircle class="w-4 h-4 shrink-0 text-destructive" />
        <span class="flex-1">{{ session.errorBanner }}</span>
        <button @click="session.dismissError" class="p-0.5 hover:bg-destructive/20 rounded transition-colors text-destructive">
          <XIcon class="w-3.5 h-3.5" />
        </button>
      </div>

      <!-- Messages -->
      <ScrollArea class="flex-1 p-4" ref="scrollAreaRef">
        <!-- Empty State -->
        <div v-if="!hasUserMessages"
          class="flex flex-col items-center justify-center h-full max-w-full md:max-w-3xl lg:max-w-4xl xl:max-w-5xl mx-auto text-center py-16 th-text">
          <div class="w-14 h-14 rounded-2xl th-gradient-brand flex items-center justify-center mb-5 shadow-lg shadow-primary/20">
            <Sparkles class="w-7 h-7 text-white" />
          </div>
          <h2 class="text-xl font-semibold th-text mb-2">How can I help you today?</h2>
          <p class="th-text-muted mb-6 text-xs">Ask me anything about your code, project, or ideas.</p>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-2 w-full">
            <button v-for="(prompt, idx) in suggestedPrompts" :key="idx" @click="sendPrompt(prompt.text)"
              :disabled="!session.isConnected"
              class="flex items-center gap-2 p-3 th-surface th-border th-hover rounded-xl text-left text-xs th-text-secondary hover:th-text transition-all duration-200 disabled:opacity-40 disabled:cursor-not-allowed group shadow-sm">
              <span class="text-base flex-shrink-0 group-hover:scale-110 transition-transform">{{ prompt.icon }}</span>
              <span>{{ prompt.text }}</span>
            </button>
          </div>
        </div>

        <!-- Messages List -->
        <div v-else class="flex flex-col gap-1 max-w-full md:max-w-4xl lg:max-w-5xl xl:max-w-6xl mx-auto w-full pb-4 px-4">
          <template v-for="(msg, idx) in messages" :key="msg.id">
            <ChatTimeDivider
              v-if="idx > 0 && msg.timestamp - messages[idx - 1].timestamp > 5 * 60 * 1000"
              :timestamp="msg.timestamp"
            />
            <MessageBubble
              :message="msg"
              :is-last-bot-message="msg.role === 'bot' && idx === messages.length - 1"
              :is-thinking="session.isThinking"
              :is-receiving="session.isReceiving"
              @retry="() => retryMessage(msg.id, msg.content)"
            />
          </template>

          <!-- Typing indicator -->
          <div v-if="session.isReceiving && !session.isThinking" class="flex items-end gap-2 mt-2 ml-1">
            <div class="w-7 h-7 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center shrink-0">
              <Bot class="w-3.5 h-3.5 text-white" />
            </div>
            <div class="px-3 py-2 rounded-2xl rounded-bl-sm th-typing-bg th-text-secondary text-xs flex items-center gap-1">
              <span class="w-1.5 h-1.5 bg-muted-foreground rounded-full animate-bounce" style="animation-delay: 0ms" />
              <span class="w-1.5 h-1.5 bg-muted-foreground rounded-full animate-bounce" style="animation-delay: 150ms" />
              <span class="w-1.5 h-1.5 bg-muted-foreground rounded-full animate-bounce" style="animation-delay: 300ms" />
            </div>
          </div>

          <SubagentPanel
            v-if="session.hasActiveSubagents"
            :subagents="session.activeSubagents"
            class="max-w-full md:max-w-4xl lg:max-w-5xl xl:max-w-6xl mx-auto w-full mt-2"
          />
        </div>
      </ScrollArea>

      <!-- Scroll to bottom button -->
      <Transition enter-active-class="transition-all duration-200 ease-out" leave-active-class="transition-all duration-150 ease-in"
        enter-from-class="opacity-0 translate-y-2" leave-to-class="opacity-0 translate-y-2">
        <button v-if="showScrollButton" @click="forceScrollToBottom"
          class="absolute bottom-28 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1.5 px-3 py-1.5 bg-popover hover:bg-accent border border-border rounded-full text-foreground text-xs shadow-lg backdrop-blur-sm transition-colors">
          <ArrowDown class="w-3.5 h-3.5" />
          New messages
        </button>
      </Transition>

      <!-- Input Area -->
      <div class="p-4 bg-transparent shrink-0">
        <div class="max-w-full md:max-w-4xl lg:max-w-5xl xl:max-w-6xl mx-auto w-full relative">
          <div class="flex items-end th-input-bg border th-border rounded-2xl p-2 shadow-xl backdrop-blur-xl transition-all"
            :class="{
              'focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/20': session.sessionStatus === 'idle' || session.sessionStatus === 'disconnected',
              'border-primary/30 ring-2 ring-primary/20': session.sessionStatus === 'receiving' || session.sessionStatus === 'sending'
            }">
            <textarea ref="inputRef" v-model="inputValue" @keydown="handleKeydown" @input="autoResize"
              :placeholder="session.sessionStatus === 'receiving' ? 'AI is processing...' : 'Type a message...'"
              :disabled="!session.isConnected || session.sessionStatus === 'receiving' || session.sessionStatus === 'sending'"
              autofocus rows="1"
              class="flex-1 overflow-x-hidden border-0 bg-transparent shadow-none focus:outline-none focus:ring-0 th-text px-3 py-2.5 disabled:opacity-50 disabled:cursor-not-allowed resize-none custom-scrollbar min-h-[40px] max-h-[200px]"></textarea>

            <Button v-if="session.sessionStatus === 'receiving' || session.isThinking" @click="session.stopGenerating"
              class="w-9 h-9 rounded-xl bg-destructive/80 hover:bg-destructive text-white shrink-0 ml-2 transition-all" size="icon" title="Stop generating">
              <Square class="w-3.5 h-3.5 fill-current" />
            </Button>
            <Button v-else @click="submit" :disabled="!inputValue.trim() || !session.isConnected || session.sessionStatus === 'sending'"
              class="w-9 h-9 rounded-xl text-white shrink-0 ml-2 transition-all"
              :class="{ 'bg-primary hover:opacity-90': session.sessionStatus === 'idle', 'bg-muted cursor-not-allowed': session.sessionStatus !== 'idle' }"
              size="icon">
              <Send class="w-4 h-4" />
            </Button>
          </div>
          <div class="flex items-center justify-center text-[10px] th-text-dim mt-2 px-1">
            <span>Shift+Enter for new line</span>
          </div>
        </div>
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
