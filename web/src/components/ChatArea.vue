<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { useChatStore } from '@/stores/chatStore';
import { Menu as HeadlessMenu, MenuButton, MenuItem, MenuItems } from '@headlessui/vue';
import { AlertCircle, ArrowDown, Bot, Cpu, Loader2, MoreVertical, RotateCcw, Send, Sparkles, Square, Trash2, X as XIcon } from 'lucide-vue-next';
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { useIMWebSocket } from '../hooks/useIMWebSocket';
import type { Message, SubagentState } from '../types';
import ChatTimeDivider from './ChatTimeDivider.vue';
import MessageBubble from './MessageBubble.vue';
import SubagentPanel from './SubagentPanel.vue';

const props = defineProps<{
  chatId: string;
}>();

const chatStore = useChatStore();
const scrollAreaRef = ref<InstanceType<typeof ScrollArea> | null>(null);
const inputRef = ref<HTMLTextAreaElement | null>(null);
const inputValue = ref('');

const isThinking = ref(false);
const isSending = ref(false);
const isReceiving = ref(false);
const toolStartTimes = ref<Record<string, number>>({});
const activeSubagents = ref<Map<string, SubagentState>>(new Map());
const hasActiveSubagents = computed(() => activeSubagents.value.size > 0);

const errorBanner = ref<string | null>(null);
let errorBannerTimer: ReturnType<typeof setTimeout> | null = null;

const showScrollButton = ref(false);
const userScrolledUp = ref(false);

const messages = computed(() => chatStore.activeMessages);

// Context stats
const contextStats = computed(() => chatStore.activeChat?.contextStats);
const watermarkInfo = computed(() => chatStore.activeChat?.watermarkInfo);

const usageColor = computed(() => {
  const pct = contextStats.value?.usage_percent || 0;
  if (pct < 80) return 'bg-emerald-500';
  if (pct < 100) return 'bg-amber-500';
  return 'bg-red-500';
});

const suggestedPrompts = [
  { icon: '💡', text: 'Explain how this project is structured' },
  { icon: '🔍', text: 'Help me find and fix bugs in my code' },
  { icon: '📝', text: 'Write a unit test for a function' },
  { icon: '🚀', text: 'Suggest performance improvements' },
];

// Scroll helpers
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

const setupScrollObserver = () => {
  nextTick(() => {
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (!scrollEl) return;
    scrollEl.addEventListener('scroll', () => {
      const distFromBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight;
      const nearBottom = distFromBottom < 100;
      showScrollButton.value = distFromBottom > 200;
      userScrolledUp.value = !nearBottom;
    });
  });
};

// WebSocket message handling
const activeChatIdForSocket = computed(() => props.chatId);

const handleSubagentStarted = (msg: { id: string; task: string; index: number }) => {
  activeSubagents.value.set(msg.id, {
    id: msg.id,
    index: msg.index,
    task: msg.task,
    status: 'running',
    toolCalls: [],
    toolCount: 0,
    startTime: Date.now(),
  });
};

const handleSubagentThinking = (msg: { id: string; content: string }) => {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) subagent.thinking = (subagent.thinking || '') + msg.content;
};

const handleSubagentContent = (msg: { id: string; content: string }) => {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) subagent.content = (subagent.content || '') + msg.content;
};

const handleSubagentToolStart = (msg: { id: string; name: string; arguments?: string }) => {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    const toolId = Date.now().toString() + '_' + Math.random().toString(36).substr(2, 9);
    subagent.toolCalls.push({ id: toolId, name: msg.name, arguments: msg.arguments, status: 'running', output: null });
    subagent.toolCount++;
    (subagent as any)._toolIdMap = (subagent as any)._toolIdMap || {};
    (subagent as any)._toolIdMap[msg.name + '_' + Date.now()] = toolId;
  }
};

const handleSubagentToolEnd = (msg: { id: string; tool_id?: string; name: string; output?: string }) => {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent && subagent.toolCalls.length > 0) {
    let tool: any | undefined;
    if (msg.tool_id) {
      tool = subagent.toolCalls.find(t => t.id === msg.tool_id);
    }
    if (!tool) {
      tool = [...subagent.toolCalls].reverse().find(t => t.name === msg.name && t.status === 'running');
    }
    if (tool) {
      tool.status = 'complete';
      tool.output = msg.output;
      const elapsed = Date.now() - parseInt(tool.id.split('_')[0]);
      tool.duration = (elapsed / 1000).toFixed(1) + 's';
    }
  }
};

const handleSubagentCompleted = (msg: { id: string; index: number; summary: string; tool_count: number }, botMsg: Message) => {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    subagent.status = 'completed';
    subagent.summary = msg.summary;
    subagent.toolCount = msg.tool_count;
    subagent.endTime = Date.now();
  }
  checkAndFinalizeSubagents(botMsg);
};

const handleSubagentError = (msg: { id: string; index: number; error: string }, botMsg: Message) => {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    subagent.status = 'error';
    subagent.error = msg.error;
    subagent.endTime = Date.now();
  }
  checkAndFinalizeSubagents(botMsg);
};

const checkAndFinalizeSubagents = (botMsg: Message) => {
  const allCompleted = [...activeSubagents.value.values()].every(s => s.status !== 'running');
  if (allCompleted && activeSubagents.value.size > 0) {
    finalizeSubagents(botMsg);
  }
};

const finalizeSubagents = (botMsg: Message) => {
  const subagents = [...activeSubagents.value.values()].sort((a, b) => a.index - b.index);
  if (subagents.length === 0) return;
  const summary = generateSubagentSummary(subagents);
  chatStore.appendToMessage(props.chatId, botMsg.id, summary, 'content');
  activeSubagents.value.clear();
};

const generateSubagentSummary = (subagents: SubagentState[]): string => {
  const lines = ['\n\n---\n**✅ 并行任务完成**\n'];
  for (const s of subagents) {
    const status = s.status === 'error' ? '❌' : '✓';
    const duration = s.endTime ? ((s.endTime - s.startTime) / 1000).toFixed(1) : '?';
    lines.push(`${status} **Task ${s.index}**: ${s.summary || s.task} _(${s.toolCount} 工具, ${duration}s)_`);
  }
  return lines.join('\n');
};

const processWebSocketMessage = (msg: any) => {
  isSending.value = false;
  isReceiving.value = true;

  let botMsg = messages.value[messages.value.length - 1];
  if (!botMsg || botMsg.role !== 'bot') {
    chatStore.appendMessage(props.chatId, {
      id: Date.now().toString(),
      role: 'bot',
      content: '',
      timestamp: Date.now()
    });
    nextTick(() => {
      botMsg = messages.value[messages.value.length - 1];
      processWebSocketMessageInner(msg, botMsg);
    });
    return;
  }

  processWebSocketMessageInner(msg, botMsg);
};

const processWebSocketMessageInner = (msg: any, botMsg: Message) => {
  switch (msg.type) {
    case 'thinking':
      isThinking.value = true;
      chatStore.appendToMessage(props.chatId, botMsg.id, msg.content, 'thinking');
      break;
    case 'tool_start':
      isThinking.value = true;
      chatStore.ensureToolCalls(props.chatId, botMsg.id);
      const toolId = Date.now().toString() + '_' + Math.random().toString(36).substr(2, 9);
      chatStore.pushToolCall(props.chatId, botMsg.id, {
        id: toolId,
        name: msg.name,
        arguments: msg.arguments || '',
        status: 'running',
        result: null
      });
      toolStartTimes.value[toolId] = Date.now();
      break;
    case 'tool_end':
      isThinking.value = true;
      const toolCalls = messages.value.find(m => m.id === botMsg.id)?.toolCalls;
      if (toolCalls && toolCalls.length > 0) {
        const matchingTool = [...toolCalls].reverse().find(t => t.name === msg.name && t.status === 'running');
        const activeTool = matchingTool || [...toolCalls].reverse().find(t => t.status === 'running') || toolCalls[toolCalls.length - 1];
        const updates: any = { status: msg.error ? 'error' : 'complete', result: msg.error || msg.output };
        if (toolStartTimes.value[activeTool.id]) {
          updates.duration = ((Date.now() - toolStartTimes.value[activeTool.id]) / 1000).toFixed(1);
          delete toolStartTimes.value[activeTool.id];
        }
        chatStore.updateToolCall(props.chatId, botMsg.id, activeTool.id, updates);
      }
      break;
    case 'content':
    case 'text':
      isThinking.value = false;
      chatStore.appendToMessage(props.chatId, botMsg.id, msg.content, 'content');
      break;
    case 'error':
      isThinking.value = false;
      showError(msg.content || msg.message || 'An error occurred');
      break;
    case 'done':
      isThinking.value = false;
      isReceiving.value = false;
      fetchContext();
      setTimeout(() => scrollToBottom(true), 150);
      break;
    case 'subagent_started':
      handleSubagentStarted(msg);
      break;
    case 'subagent_thinking':
      handleSubagentThinking(msg);
      break;
    case 'subagent_content':
      handleSubagentContent(msg);
      break;
    case 'subagent_tool_start':
      handleSubagentToolStart(msg);
      break;
    case 'subagent_tool_end':
      handleSubagentToolEnd(msg);
      break;
    case 'subagent_completed':
      handleSubagentCompleted(msg, botMsg);
      break;
    case 'subagent_error':
      handleSubagentError(msg, botMsg);
      break;
  }
};

const handleMessage = (data: string) => {
  try {
    const msg = JSON.parse(data);
    processWebSocketMessage(msg);
  } catch (e) {
    isThinking.value = false;
    isSending.value = false;
    const lastMsg = messages.value[messages.value.length - 1];
    if (lastMsg && lastMsg.role === 'bot') {
      chatStore.appendToMessage(props.chatId, lastMsg.id, data, 'content');
    }
  }
  scrollToBottom();
};

const { isConnected, showReconnectButton, connect, manualReconnect, send } = useIMWebSocket(activeChatIdForSocket, handleMessage);

type SessionStatus = 'disconnected' | 'idle' | 'sending' | 'receiving';
const sessionStatus = computed<SessionStatus>(() => {
  if (!isConnected.value) return 'disconnected';
  if (isSending.value) return 'sending';
  if (isReceiving.value || isThinking.value) return 'receiving';
  return 'idle';
});

const hasUserMessages = computed(() => messages.value.some(m => m.role === 'user' || m.role === 'bot'));

onMounted(() => {
  connect();
  fetchContext();
  nextTick(() => scrollToBottom(true));
  setupScrollObserver();
});

watch(() => props.chatId, () => {
  connect();
  fetchContext();
  userScrolledUp.value = false;
  nextTick(() => scrollToBottom(true));
});

watch(() => messages.value.length, () => {
  scrollToBottom();
});

const showError = (message: string) => {
  errorBanner.value = message;
  if (errorBannerTimer) clearTimeout(errorBannerTimer);
  errorBannerTimer = setTimeout(() => { errorBanner.value = null; }, 8000);
};

const dismissError = () => {
  errorBanner.value = null;
  if (errorBannerTimer) clearTimeout(errorBannerTimer);
};

const sendMessage = () => {
  if (!inputValue.value.trim() || !isConnected.value || isSending.value || isReceiving.value) return;

  const text = inputValue.value;
  inputValue.value = '';

  const msgId = Date.now().toString();
  chatStore.appendMessage(props.chatId, {
    id: msgId,
    role: 'user',
    content: text,
    timestamp: Date.now(),
    status: 'sending'
  });

  isSending.value = true;
  try {
    send(text);
    chatStore.updateMessageStatus(props.chatId, msgId, 'sent');
  } catch (e) {
    chatStore.updateMessageStatus(props.chatId, msgId, 'error');
  }

  nextTick(() => {
    inputRef.value?.focus();
    if (inputRef.value) inputRef.value.style.height = 'auto';
  });
};

const retryMessage = (msgId: string, content: string) => {
  if (!isConnected.value) return;
  chatStore.updateMessageStatus(props.chatId, msgId, 'sending');
  try {
    send(content);
    chatStore.updateMessageStatus(props.chatId, msgId, 'sent');
  } catch (e) {
    chatStore.updateMessageStatus(props.chatId, msgId, 'error');
  }
};

const stopGenerating = () => {
  send(JSON.stringify({ type: 'cancel' }));
  isThinking.value = false;
  isReceiving.value = false;
  isSending.value = false;

  const lastMsg = messages.value[messages.value.length - 1];
  if (lastMsg && lastMsg.role === 'bot' && lastMsg.toolCalls) {
    lastMsg.toolCalls.forEach(tc => {
      if (tc.status === 'running') chatStore.updateToolCall(props.chatId, lastMsg.id, tc.id, { status: 'error' });
    });
  }
};

const sendPrompt = (prompt: string) => {
  inputValue.value = prompt;
  sendMessage();
};

const handleKeydown = (event: KeyboardEvent) => {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    sendMessage();
  }
};

const autoResize = () => {
  const el = inputRef.value;
  if (!el) return;
  el.style.height = 'auto';
  el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
};

const clearHistory = () => {
  chatStore.clearChatMessages(props.chatId);
};

const isCompacting = ref(false);

const fetchContext = async () => {
  try {
    const baseUrl = import.meta.env.VITE_API_URL || 'http://localhost:3000';
    const sessionKey = encodeURIComponent(`websocket:${props.chatId}`);
    const res = await fetch(`${baseUrl}/api/sessions/${sessionKey}/context`);
    const data = await res.json();
    if (res.ok && data.context_stats) {
      chatStore.setContextStats(props.chatId, data.context_stats);
    }
    if (res.ok && data.watermark_info) {
      chatStore.setWatermarkInfo(props.chatId, data.watermark_info);
    }
  } catch (e) {
    console.error('Fetch context failed:', e);
  }
};

const forceCompact = async () => {
  if (isCompacting.value) return;
  isCompacting.value = true;
  try {
    const baseUrl = import.meta.env.VITE_API_URL || 'http://localhost:3000';
    const sessionKey = encodeURIComponent(`websocket:${props.chatId}`);
    const res = await fetch(`${baseUrl}/api/sessions/${sessionKey}/context/compact`, { method: 'POST' });
    const data = await res.json();
    if (res.ok && data.context_stats) {
      chatStore.setContextStats(props.chatId, data.context_stats);
    }
    if (res.ok && data.watermark_info) {
      chatStore.setWatermarkInfo(props.chatId, data.watermark_info);
    }
  } catch (e) {
    console.error('Force compact failed:', e);
  } finally {
    isCompacting.value = false;
  }
};
</script>

<template>
  <div class="flex h-full w-full relative">
    <div class="flex flex-col flex-1 min-w-0">
      <!-- Header -->
      <header class="py-3 px-5 bg-white/80 dark:bg-slate-800/80 border-b border-gray-200 dark:border-white/10 flex justify-between items-center shrink-0">
        <div class="flex items-center gap-3">
          <div class="w-9 h-9 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center">
            <Bot class="w-5 h-5 text-white" />
          </div>
          <div>
            <div class="text-sm font-semibold text-gray-900 dark:text-slate-100">Gasket</div>
            <div class="text-[10px] text-gray-500 dark:text-slate-400 flex items-center gap-1">
              <span class="w-1.5 h-1.5 rounded-full" :class="isConnected ? 'bg-emerald-500' : 'bg-red-500'" />
              {{ isConnected ? 'Online' : 'Offline' }}
            </div>
          </div>
        </div>

        <div class="flex items-center gap-2">
          <div
            v-if="sessionStatus !== 'idle'"
            class="flex items-center gap-1.5 px-2.5 py-1 rounded-full text-[11px] font-medium border animate-in fade-in zoom-in-95 duration-200"
            :class="{
              'bg-red-500/10 text-red-500 dark:text-red-400 border-red-500/20': sessionStatus === 'disconnected',
              'bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20': sessionStatus === 'sending',
              'bg-violet-500/10 text-violet-600 dark:text-violet-400 border-violet-500/20': sessionStatus === 'receiving'
            }"
          >
            <Loader2 v-if="sessionStatus === 'sending' || sessionStatus === 'receiving'" class="w-3.5 h-3.5 animate-spin" />
            <span v-if="sessionStatus === 'disconnected'">Disconnected</span>
            <span v-else-if="sessionStatus === 'sending'">Sending...</span>
            <span v-else-if="sessionStatus === 'receiving'">Thinking...</span>
          </div>

          <Button v-if="showReconnectButton" variant="outline" size="sm" @click="manualReconnect"
            class="text-amber-600 dark:text-amber-400 border-amber-500/30 hover:bg-amber-500/10 text-xs h-8">
            <RotateCcw class="w-3.5 h-3.5 mr-1.5" />
            Reconnect
          </Button>

          <HeadlessMenu as="div" class="relative">
            <MenuButton as="button" class="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-white/10 text-gray-500 dark:text-slate-400 hover:text-gray-800 dark:hover:text-slate-200 transition-colors">
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
              <MenuItems class="absolute right-0 top-10 z-30 w-40 origin-top-right rounded-lg bg-white dark:bg-slate-800 border border-gray-200 dark:border-white/10 shadow-lg focus:outline-none py-1">
                <MenuItem v-slot="{ active }">
                  <button @click="clearHistory" :class="[active ? 'bg-gray-100 dark:bg-white/10' : '', 'group flex w-full items-center px-3 py-2 text-xs text-gray-700 dark:text-slate-300']">
                    <Trash2 class="w-3.5 h-3.5 mr-2 text-gray-400 dark:text-slate-400" />
                    Clear History
                  </button>
                </MenuItem>
              </MenuItems>
            </transition>
          </HeadlessMenu>
        </div>
      </header>

      <!-- Context Stats Bar -->
      <div v-if="contextStats" class="px-4 py-2 bg-gray-100/60 dark:bg-slate-900/40 border-b border-gray-200 dark:border-white/5 flex items-center gap-3 shrink-0">
        <div class="text-[10px] text-gray-600 dark:text-slate-400 font-medium whitespace-nowrap">
          Context: {{ contextStats.usage_percent.toFixed(1) }}%
        </div>
        <div class="flex-1 h-1.5 bg-gray-300 dark:bg-slate-700 rounded-full overflow-hidden relative">
          <div class="h-full rounded-full transition-all duration-500" :class="usageColor" :style="{ width: Math.min(contextStats.usage_percent, 100) + '%' }" />
        </div>
        <div v-if="watermarkInfo" class="text-[10px] text-gray-500 dark:text-slate-500 whitespace-nowrap">
          Watermark: {{ watermarkInfo.watermark }}/{{ watermarkInfo.max_sequence }}
        </div>
        <Button variant="outline" size="sm" class="h-6 text-[10px] px-2 bg-white dark:bg-white/5 border-gray-200 dark:border-white/10 hover:bg-gray-100 dark:hover:bg-white/10 text-gray-700 dark:text-slate-300"
          :disabled="isCompacting" @click="forceCompact">
          <Cpu v-if="!isCompacting" class="w-3 h-3 mr-1" />
          <Loader2 v-else class="w-3 h-3 mr-1 animate-spin" />
          {{ isCompacting ? 'Compressing...' : 'Compress' }}
        </Button>
      </div>

      <!-- Error Banner -->
      <div v-if="errorBanner"
        class="mx-4 mt-2 flex items-center gap-2 bg-red-500/15 border border-red-500/30 text-red-600 dark:text-red-300 px-3 py-2 rounded-lg text-xs animate-in fade-in slide-in-from-top-2 duration-300 shrink-0">
        <AlertCircle class="w-4 h-4 shrink-0 text-red-500 dark:text-red-400" />
        <span class="flex-1">{{ errorBanner }}</span>
        <button @click="dismissError" class="p-0.5 hover:bg-red-500/20 rounded transition-colors">
          <XIcon class="w-3.5 h-3.5" />
        </button>
      </div>

      <!-- Messages -->
      <ScrollArea class="flex-1 p-4" ref="scrollAreaRef">
        <!-- Empty State -->
        <div v-if="!hasUserMessages"
          class="flex flex-col items-center justify-center h-full max-w-2xl mx-auto text-center py-16">
          <div class="w-14 h-14 rounded-2xl bg-gradient-to-br from-blue-500 to-violet-600 flex items-center justify-center mb-5 shadow-lg shadow-blue-500/20">
            <Sparkles class="w-7 h-7 text-white" />
          </div>
          <h2 class="text-xl font-semibold text-gray-900 dark:text-slate-100 mb-2">How can I help you today?</h2>
          <p class="text-gray-500 dark:text-slate-400 mb-6 text-xs">Ask me anything about your code, project, or ideas.</p>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-2 w-full">
            <button v-for="(prompt, idx) in suggestedPrompts" :key="idx" @click="sendPrompt(prompt.text)"
              :disabled="!isConnected"
              class="flex items-center gap-2 p-3 bg-white dark:bg-slate-800/60 border border-gray-200 dark:border-white/5 hover:border-gray-300 dark:hover:border-white/15 rounded-xl text-left text-xs text-gray-700 dark:text-slate-300 hover:text-gray-900 dark:hover:text-slate-100 transition-all duration-200 disabled:opacity-40 disabled:cursor-not-allowed group shadow-sm">
              <span class="text-base flex-shrink-0 group-hover:scale-110 transition-transform">{{ prompt.icon }}</span>
              <span>{{ prompt.text }}</span>
            </button>
          </div>
        </div>

        <!-- Messages List -->
        <div v-else class="flex flex-col gap-1 max-w-4xl mx-auto w-full pb-4">
          <template v-for="(msg, idx) in messages" :key="msg.id">
            <ChatTimeDivider
              v-if="idx > 0 && msg.timestamp - messages[idx - 1].timestamp > 5 * 60 * 1000"
              :timestamp="msg.timestamp"
            />
            <MessageBubble
              :message="msg"
              :is-last-bot-message="msg.role === 'bot' && idx === messages.length - 1"
              :is-thinking="isThinking"
              :is-receiving="isReceiving"
              @retry="() => retryMessage(msg.id, msg.content)"
            />
          </template>

          <!-- Typing indicator -->
          <div v-if="isReceiving && !isThinking" class="flex items-end gap-2 mt-2 ml-1">
            <div class="w-7 h-7 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center shrink-0">
              <Bot class="w-3.5 h-3.5 text-white" />
            </div>
            <div class="px-3 py-2 rounded-2xl rounded-bl-sm bg-gray-200 dark:bg-slate-700/60 text-gray-600 dark:text-slate-300 text-xs flex items-center gap-1">
              <span class="w-1.5 h-1.5 bg-gray-500 dark:bg-slate-400 rounded-full animate-bounce" style="animation-delay: 0ms" />
              <span class="w-1.5 h-1.5 bg-gray-500 dark:bg-slate-400 rounded-full animate-bounce" style="animation-delay: 150ms" />
              <span class="w-1.5 h-1.5 bg-gray-500 dark:bg-slate-400 rounded-full animate-bounce" style="animation-delay: 300ms" />
            </div>
          </div>

          <SubagentPanel
            v-if="hasActiveSubagents"
            :subagents="activeSubagents"
            class="max-w-4xl mx-auto w-full mt-2"
          />
        </div>
      </ScrollArea>

      <!-- Scroll to bottom button -->
      <Transition enter-active-class="transition-all duration-200 ease-out" leave-active-class="transition-all duration-150 ease-in"
        enter-from-class="opacity-0 translate-y-2" leave-to-class="opacity-0 translate-y-2">
        <button v-if="showScrollButton" @click="forceScrollToBottom"
          class="absolute bottom-28 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1.5 px-3 py-1.5 bg-white dark:bg-slate-700/90 hover:bg-gray-100 dark:hover:bg-slate-600/90 border border-gray-200 dark:border-white/10 rounded-full text-gray-700 dark:text-slate-300 text-xs shadow-lg backdrop-blur-sm transition-colors">
          <ArrowDown class="w-3.5 h-3.5" />
          New messages
        </button>
      </Transition>

      <!-- Input Area -->
      <div class="p-4 bg-transparent shrink-0">
        <div class="max-w-4xl mx-auto w-full relative">
          <div class="flex items-end bg-white dark:bg-slate-900/70 border border-gray-200 dark:border-white/10 rounded-2xl p-2 shadow-xl backdrop-blur-xl transition-all"
            :class="{
              'focus-within:border-blue-500/50 focus-within:ring-2 focus-within:ring-blue-500/20': sessionStatus === 'idle' || sessionStatus === 'disconnected',
              'border-violet-500/30 ring-2 ring-violet-500/20': sessionStatus === 'receiving',
              'border-blue-500/30 ring-2 ring-blue-500/20': sessionStatus === 'sending'
            }">
            <textarea ref="inputRef" v-model="inputValue" @keydown="handleKeydown" @input="autoResize"
              :placeholder="sessionStatus === 'receiving' ? 'AI is processing...' : 'Type a message...'"
              :disabled="!isConnected || sessionStatus === 'receiving' || sessionStatus === 'sending'"
              autofocus rows="1"
              class="flex-1 overflow-x-hidden border-0 bg-transparent shadow-none focus:outline-none focus:ring-0 text-gray-900 dark:text-slate-100 px-3 py-2.5 disabled:opacity-50 disabled:cursor-not-allowed resize-none custom-scrollbar min-h-[40px] max-h-[200px]"></textarea>

            <Button v-if="sessionStatus === 'receiving' || isThinking" @click="stopGenerating"
              class="w-9 h-9 rounded-xl bg-red-500/80 hover:bg-red-500 text-white shrink-0 ml-2 transition-all" size="icon" title="Stop generating">
              <Square class="w-3.5 h-3.5 fill-current" />
            </Button>
            <Button v-else @click="sendMessage" :disabled="!inputValue.trim() || !isConnected || sessionStatus === 'sending'"
              class="w-9 h-9 rounded-xl text-white shrink-0 ml-2 transition-all"
              :class="{ 'bg-blue-500 hover:bg-blue-400': sessionStatus === 'idle', 'bg-slate-300 dark:bg-slate-600 cursor-not-allowed': sessionStatus !== 'idle' }"
              size="icon">
              <Send class="w-4 h-4" />
            </Button>
          </div>
          <div class="flex items-center justify-center text-[10px] text-gray-400 dark:text-slate-500 mt-2 px-1">
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
