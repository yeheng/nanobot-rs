<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { useChatWebSocket } from '@/hooks/useChatWebSocket';
import { AlertCircle, ArrowDown, Cpu, RotateCcw, Send, Sparkles, Square, Trash2, Upload, Wifi, WifiOff, X as XIcon } from 'lucide-vue-next';
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import type { Message, ToolCall } from '../App.vue';
import type { SubagentState } from '../types';
import MessageBubble from './MessageBubble.vue';
import SubagentPanel from './SubagentPanel.vue';

// Helper function to get scrollable element from ScrollArea
const getScrollElement = (scrollArea: InstanceType<typeof ScrollArea> | null): HTMLElement | null => {
  if (!scrollArea) return null;
  const el = scrollArea.$el as HTMLElement;
  if (!el) return null;
  const viewport = el.querySelector('[data-radix-scroll-area-viewport]') as HTMLElement;
  return viewport || el;
};

const props = defineProps<{
  sessionId: string;
  messages: Message[];
}>();

// Incremental update emits
const emit = defineEmits<{
  (e: 'get-or-create-bot-msg'): Message | null;
  (e: 'append-message', message: Message): void;
  (e: 'append-to-message', messageId: string, content: string, field?: 'content' | 'thinking'): void;
  (e: 'update-message', messageId: string, updates: Partial<Message>): void;
  (e: 'ensure-tool-calls', messageId: string): void;
  (e: 'push-tool-call', messageId: string, toolCall: ToolCall): void;
  (e: 'update-tool-call', messageId: string, toolId: string, updates: Partial<ToolCall>): void;
  (e: 'clear-messages'): void;
}>();

// Refs
const scrollAreaRef = ref<InstanceType<typeof ScrollArea> | null>(null);
const inputRef = ref<HTMLTextAreaElement | null>(null);
const inputValue = ref('');

// State
const isThinking = ref(false);
const isSending = ref(false);
const isReceiving = ref(false);

// Tool timing
const toolStartTimes = ref<Record<string, number>>({});

// Subagent 状态管理
const activeSubagents = ref<Map<string, SubagentState>>(new Map());

// 计算属性：是否有活跃的 subagent
const hasActiveSubagents = computed(() => activeSubagents.value.size > 0);

// Error messages
const errorBanner = ref<string | null>(null);
let errorBannerTimer: ReturnType<typeof setTimeout> | null = null;

// UI State
const showScrollButton = ref(false);
const sendOnEnter = ref(true);

// Suggested prompts
const suggestedPrompts = [
  { icon: '💡', text: 'Explain how this project is structured' },
  { icon: '🔍', text: 'Help me find and fix bugs in my code' },
  { icon: '📝', text: 'Write a unit test for a function' },
  { icon: '🚀', text: 'Suggest performance improvements' },
];

// WebSocket handling

// === Subagent 消息处理函数 ===
function handleSubagentStarted(msg: { id: string; task: string; index: number }) {
  activeSubagents.value.set(msg.id, {
    id: msg.id,
    index: msg.index,
    task: msg.task,
    status: 'running',
    toolCalls: [],
    toolCount: 0,
    startTime: Date.now(),
  });
}

function handleSubagentThinking(msg: { id: string; content: string }) {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    subagent.thinking = (subagent.thinking || '') + msg.content;
  }
}

function handleSubagentContent(msg: { id: string; content: string }) {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    subagent.content = (subagent.content || '') + msg.content;
  }
}

function handleSubagentToolStart(msg: { id: string; name: string; arguments?: string }) {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    const toolId = Date.now().toString() + '_' + Math.random().toString(36).substr(2, 9);
    subagent.toolCalls.push({
      id: toolId,
      name: msg.name,
      arguments: msg.arguments,
      status: 'running',
      output: null,
    });
    subagent.toolCount++;
    // Store tool ID mapping for later matching
    (subagent as any)._toolIdMap = (subagent as any)._toolIdMap || {};
    (subagent as any)._toolIdMap[msg.name + '_' + Date.now()] = toolId;
  }
}

function handleSubagentToolEnd(msg: { id: string; tool_id?: string; name: string; output?: string }) {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent && subagent.toolCalls.length > 0) {
    let tool: any | undefined;
    
    // Prefer tool_id if provided by backend
    if (msg.tool_id) {
      tool = subagent.toolCalls.find(t => t.id === msg.tool_id);
    }
    
    // Fallback: find by name and running status (most recent first)
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
}

function handleSubagentCompleted(msg: { id: string; index: number; summary: string; tool_count: number }, botMsg: Message) {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    subagent.status = 'completed';
    subagent.summary = msg.summary;
    subagent.toolCount = msg.tool_count;
    subagent.endTime = Date.now();
  }
  checkAndFinalizeSubagents(botMsg);
}

function handleSubagentError(msg: { id: string; index: number; error: string }, botMsg: Message) {
  const subagent = activeSubagents.value.get(msg.id);
  if (subagent) {
    subagent.status = 'error';
    subagent.error = msg.error;
    subagent.endTime = Date.now();
  }
  checkAndFinalizeSubagents(botMsg);
}

function checkAndFinalizeSubagents(botMsg: Message) {
  const allCompleted = [...activeSubagents.value.values()]
    .every(s => s.status !== 'running');

  if (allCompleted && activeSubagents.value.size > 0) {
    finalizeSubagents(botMsg);
  }
}

function finalizeSubagents(botMsg: Message) {
  const subagents = [...activeSubagents.value.values()].sort((a, b) => a.index - b.index);
  if (subagents.length === 0) return;

  // 生成合并摘要
  const summary = generateSubagentSummary(subagents);

  // 追加到最新的 bot 消息
  emit('append-to-message', botMsg.id, summary, 'content');

  // 清理活跃状态
  activeSubagents.value.clear();
}

function generateSubagentSummary(subagents: SubagentState[]): string {
  const lines = ['\n\n---\n**✅ 并行任务完成**\n'];
  for (const s of subagents) {
    const status = s.status === 'error' ? '❌' : '✓';
    const duration = s.endTime ? ((s.endTime - s.startTime) / 1000).toFixed(1) : '?';
    lines.push(`${status} **Task ${s.index}**: ${s.summary || s.task} _(${s.toolCount} 工具, ${duration}s)_`);
  }
  return lines.join('\n');
}

const handleMessage = (data: string) => {
  try {
    const msg = JSON.parse(data);

    // Get or create bot message
    let botMsg = props.messages[props.messages.length - 1];
    if (!botMsg || botMsg.role !== 'bot') {
      emit('append-message', {
        id: Date.now().toString(),
        role: 'bot' as const,
        content: '',
        timestamp: Date.now()
      });
      nextTick(() => {
        botMsg = props.messages[props.messages.length - 1];
        processWebSocketMessage(msg, botMsg);
      });
      return;
    }

    processWebSocketMessage(msg, botMsg);
  } catch (e) {
    isThinking.value = false;
    isSending.value = false;
    const lastMsg = props.messages[props.messages.length - 1];
    if (lastMsg && lastMsg.role === 'bot') {
      emit('append-to-message', lastMsg.id, data, 'content');
    }
  }

  scrollToBottom();
};

const processWebSocketMessage = (msg: any, botMsg: Message) => {
  isSending.value = false;
  isReceiving.value = true;

  switch (msg.type) {
    case 'thinking':
      isThinking.value = true;
      // Append to thinking field directly
      emit('append-to-message', botMsg.id, msg.content, 'thinking');
      break;

    case 'tool_start':
      isThinking.value = true;
      emit('ensure-tool-calls', botMsg.id);
      const toolId = Date.now().toString() + '_' + Math.random().toString(36).substr(2, 9);
      emit('push-tool-call', botMsg.id, {
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
      const toolCalls = props.messages.find(m => m.id === botMsg.id)?.toolCalls;
      if (toolCalls && toolCalls.length > 0) {
        const matchingTool = [...toolCalls].reverse().find(t => t.name === msg.name && t.status === 'running');
        const activeTool = matchingTool || [...toolCalls].reverse().find(t => t.status === 'running') || toolCalls[toolCalls.length - 1];
        const updates: any = { status: msg.error ? 'error' : 'complete', result: msg.error || msg.output };
        if (toolStartTimes.value[activeTool.id]) {
          updates.duration = ((Date.now() - toolStartTimes.value[activeTool.id]) / 1000).toFixed(1);
          delete toolStartTimes.value[activeTool.id];
        }
        emit('update-tool-call', botMsg.id, activeTool.id, updates);
      }
      break;

    case 'content':
    case 'text':
      isThinking.value = false;
      // Append to content field directly
      emit('append-to-message', botMsg.id, msg.content, 'content');
      break;

    case 'error':
      isThinking.value = false;
      showError(msg.content || msg.message || 'An error occurred');
      break;

    case 'done':
      isThinking.value = false;
      isReceiving.value = false;
      setTimeout(() => {
        const scrollEl = getScrollElement(scrollAreaRef.value);
        if (scrollEl) {
          scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: 'smooth' });
        }
      }, 150);
      break;

    // === Subagent 消息处理 ===
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

const {
  isConnected,
  isReconnecting,
  showReconnectButton,
  reconnectAttempts,
  connect,
  manualReconnect,
  send
} = useChatWebSocket(props.sessionId, handleMessage);

// Computed status
type SessionStatus = 'disconnected' | 'idle' | 'sending' | 'receiving';
const sessionStatus = computed<SessionStatus>(() => {
  if (!isConnected.value) return 'disconnected';
  if (isSending.value) return 'sending';
  if (isReceiving.value || isThinking.value) return 'receiving';
  return 'idle';
});

const statusConfig = computed(() => {
  const configs: Record<SessionStatus, { text: string; color: string; bgColor: string; icon: any; animate: boolean }> = {
    disconnected: {
      text: isReconnecting.value ? `Reconnecting (${reconnectAttempts.value}/5)...` : 'Disconnected',
      color: 'text-red-400',
      bgColor: 'bg-red-500',
      icon: isReconnecting.value ? RotateCcw : WifiOff,
      animate: isReconnecting.value
    },
    idle: {
      text: 'Ready to chat',
      color: 'text-emerald-400',
      bgColor: 'bg-emerald-500',
      icon: Wifi,
      animate: false
    },
    sending: {
      text: 'Sending message...',
      color: 'text-blue-400',
      bgColor: 'bg-blue-500',
      icon: Upload,
      animate: true
    },
    receiving: {
      text: 'AI is thinking...',
      color: 'text-violet-400',
      bgColor: 'bg-violet-500',
      icon: Cpu,
      animate: true
    }
  };
  return configs[sessionStatus.value];
});

// Check if user has any real messages
const hasUserMessages = computed(() => {
  return props.messages.some(m => m.role === 'user' || m.role === 'bot');
});

// Platform detection
const isMac = computed(() => navigator.platform.toUpperCase().indexOf('MAC') >= 0);
const sendShortcut = computed(() => sendOnEnter.value ? 'Enter' : (isMac.value ? '⌘+Enter' : 'Ctrl+Enter'));

// Lifecycle
onMounted(() => {
  connect();
  nextTick(() => scrollToBottom());
  setupScrollObserver();
});

watch(() => props.sessionId, () => {
  connect();
  nextTick(() => scrollToBottom());
});

// Watch message length changes for auto-scroll (avoid deep watch for performance)
watch(() => props.messages.length, () => {
  nextTick(() => scrollToBottom());
});

// Methods
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

  emit('append-message', {
    id: Date.now().toString(),
    role: 'user' as const,
    content: text,
    timestamp: Date.now()
  });

  isSending.value = true;
  send(text);

  nextTick(() => {
    inputRef.value?.focus();
    if (inputRef.value) inputRef.value.style.height = 'auto';
  });
};

const stopGenerating = () => {
  send(JSON.stringify({ type: 'cancel' }));
  isThinking.value = false;
  isReceiving.value = false;
  isSending.value = false;

  const lastMsg = props.messages[props.messages.length - 1];
  if (lastMsg && lastMsg.role === 'bot' && lastMsg.toolCalls) {
    lastMsg.toolCalls.forEach(tc => {
      if (tc.status === 'running') emit('update-tool-call', lastMsg.id, tc.id, { status: 'error' });
    });
  }
};

const sendPrompt = (prompt: string) => {
  inputValue.value = prompt;
  sendMessage();
};

const handleKeydown = (event: KeyboardEvent) => {
  if (sendOnEnter.value) {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      sendMessage();
    }
  } else {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      sendMessage();
    }
  }
};

const autoResize = () => {
  const el = inputRef.value;
  if (!el) return;
  el.style.height = 'auto';
  el.style.height = `${el.scrollHeight}px`;
};

const setupScrollObserver = () => {
  nextTick(() => {
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (!scrollEl) return;
    scrollEl.addEventListener('scroll', () => {
      const distFromBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight;
      showScrollButton.value = distFromBottom > 200;
    });
  });
};

const scrollToBottom = async () => {
  await nextTick();
  const scrollEl = getScrollElement(scrollAreaRef.value);
  if (!scrollEl) return;
  if (isReceiving.value) {
    const isNearBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 150;
    if (isNearBottom) scrollEl.scrollTop = scrollEl.scrollHeight;
  } else {
    scrollEl.scrollTop = scrollEl.scrollHeight;
  }
};

const forceScrollToBottom = () => {
  const scrollEl = getScrollElement(scrollAreaRef.value);
  if (scrollEl) scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: 'smooth' });
};

const clearHistory = () => {
  emit('clear-messages');
};
</script>

<template>
  <div class="flex h-full w-full relative">
    <div class="flex flex-col flex-1 min-w-0">
      <!-- Header -->
      <header class="py-4 px-6 bg-slate-800/80 border-b border-white/10 flex justify-between items-center">
        <Button variant="ghost" size="sm" @click="clearHistory" :disabled="messages.length === 0"
          class="text-slate-400 hover:text-red-400 hover:bg-red-500/10 disabled:opacity-30">
          <Trash2 class="w-4 h-4 mr-2" />
          Clear History
        </Button>
        <div class="flex items-center gap-3">
          <Button v-if="showReconnectButton" variant="outline" size="sm" @click="manualReconnect"
            class="text-amber-400 border-amber-500/30 hover:bg-amber-500/10 text-xs">
            <RotateCcw class="w-3.5 h-3.5 mr-1.5" />
            Reconnect
          </Button>
          <div class="flex items-center bg-black/30 px-3 py-1.5 rounded-full border border-white/5 transition-all"
            :class="{ 'animate-pulse': statusConfig.animate }">
            <div class="w-2 h-2 rounded-full mr-2 transition-all"
              :class="[statusConfig.bgColor, { 'animate-ping': statusConfig.animate }]"
              style="box-shadow: 0 0 8px currentColor;"></div>
            <component :is="statusConfig.icon" class="w-3.5 h-3.5 mr-1.5"
              :class="[statusConfig.color, { 'animate-spin': statusConfig.animate && (sessionStatus === 'receiving' || isReconnecting) }]" />
            <span class="text-xs font-medium" :class="statusConfig.color">{{ statusConfig.text }}</span>
          </div>
        </div>
      </header>

      <!-- Error Banner -->
      <div v-if="errorBanner"
        class="mx-6 mt-3 flex items-center gap-2 bg-red-500/15 border border-red-500/30 text-red-300 px-4 py-2.5 rounded-lg text-sm animate-in fade-in slide-in-from-top-2 duration-300">
        <AlertCircle class="w-4 h-4 shrink-0 text-red-400" />
        <span class="flex-1">{{ errorBanner }}</span>
        <button @click="dismissError" class="p-0.5 hover:bg-red-500/20 rounded transition-colors">
          <XIcon class="w-3.5 h-3.5" />
        </button>
      </div>

      <!-- Messages -->
      <ScrollArea class="flex-1 p-6" ref="scrollAreaRef">
        <!-- Empty State -->
        <div v-if="!hasUserMessages"
          class="flex flex-col items-center justify-center h-full max-w-2xl mx-auto text-center py-20">
          <div class="w-16 h-16 rounded-2xl bg-gradient-to-br from-blue-500 to-violet-600 flex items-center justify-center mb-6 shadow-lg shadow-blue-500/20">
            <Sparkles class="w-8 h-8 text-white" />
          </div>
          <h2 class="text-2xl font-semibold text-slate-100 mb-2">How can I help you today?</h2>
          <p class="text-slate-400 mb-8 text-sm">Ask me anything about your code, project, or ideas.</p>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 w-full">
            <button v-for="(prompt, idx) in suggestedPrompts" :key="idx" @click="sendPrompt(prompt.text)"
              :disabled="!isConnected"
              class="flex items-center gap-3 p-4 bg-slate-800/60 hover:bg-slate-700/60 border border-white/5 hover:border-white/15 rounded-xl text-left text-sm text-slate-300 hover:text-slate-100 transition-all duration-200 disabled:opacity-40 disabled:cursor-not-allowed group">
              <span class="text-lg flex-shrink-0 group-hover:scale-110 transition-transform">{{ prompt.icon }}</span>
              <span>{{ prompt.text }}</span>
            </button>
          </div>
        </div>

        <!-- Messages List -->
        <div v-else class="flex flex-col gap-6 max-w-4xl mx-auto w-full pb-4">
          <MessageBubble
            v-for="(msg, idx) in messages"
            :key="msg.id"
            :message="msg"
            :is-last-bot-message="msg.role === 'bot' && idx === messages.length - 1"
            :is-thinking="isThinking"
            :is-receiving="isReceiving"
          />

          <!-- Subagent 实时状态面板 -->
          <SubagentPanel
            v-if="hasActiveSubagents"
            :subagents="activeSubagents"
            class="max-w-4xl mx-auto w-full"
          />
        </div>
      </ScrollArea>

      <!-- Scroll to bottom button -->
      <Transition enter-active-class="transition-all duration-200 ease-out" leave-active-class="transition-all duration-150 ease-in"
        enter-from-class="opacity-0 translate-y-2" leave-to-class="opacity-0 translate-y-2">
        <button v-if="showScrollButton" @click="forceScrollToBottom"
          class="absolute bottom-36 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1.5 px-3 py-1.5 bg-slate-700/90 hover:bg-slate-600/90 border border-white/10 rounded-full text-slate-300 text-xs shadow-lg backdrop-blur-sm transition-colors">
          <ArrowDown class="w-3.5 h-3.5" />
          Scroll to bottom
        </button>
      </Transition>

      <!-- Input Area -->
      <div class="p-6 pt-0 bg-transparent shrink-0">
        <div class="max-w-4xl mx-auto w-full relative">
          <div class="flex items-end bg-slate-900/70 border border-white/10 rounded-2xl p-2 shadow-xl backdrop-blur-xl transition-all"
            :class="{
              'focus-within:border-blue-500/50 focus-within:ring-2 focus-within:ring-blue-500/20': sessionStatus === 'idle' || sessionStatus === 'disconnected',
              'border-violet-500/30 ring-2 ring-violet-500/20': sessionStatus === 'receiving',
              'border-blue-500/30 ring-2 ring-blue-500/20': sessionStatus === 'sending'
            }">
            <textarea ref="inputRef" v-model="inputValue" @keydown="handleKeydown" @input="autoResize"
              :placeholder="sessionStatus === 'receiving' ? 'AI is processing your request...' : sessionStatus === 'sending' ? 'Sending your message...' : (messages.length > 0 ? `Ready for your next prompt... (${sendShortcut} to send)` : `Type your message... (${sendShortcut} to send)`)"
              :disabled="!isConnected || sessionStatus === 'receiving' || sessionStatus === 'sending'"
              autofocus rows="1"
              class="flex-1 overflow-x-hidden border-0 bg-transparent shadow-none focus:outline-none focus:ring-0 text-slate-100 px-3 py-2.5 disabled:opacity-50 disabled:cursor-not-allowed resize-none custom-scrollbar min-h-[44px] max-h-[400px]"></textarea>

            <Button v-if="sessionStatus === 'receiving' || isThinking" @click="stopGenerating"
              class="w-11 h-11 rounded-xl bg-red-500/80 hover:bg-red-500 text-white shrink-0 ml-2 transition-all" size="icon" title="Stop generating">
              <Square class="w-4 h-4 fill-current" />
            </Button>
            <Button v-else @click="sendMessage" :disabled="!inputValue.trim() || !isConnected || sessionStatus === 'sending'"
              class="w-11 h-11 rounded-xl text-white shrink-0 ml-2 transition-all"
              :class="{ 'bg-blue-500 hover:bg-blue-400': sessionStatus === 'idle', 'bg-slate-600 cursor-not-allowed': sessionStatus !== 'idle' }"
              size="icon">
              <Send class="w-5 h-5" />
            </Button>
          </div>
          <div class="flex items-center justify-between text-xs text-slate-500 mt-3 px-1">
            <span class="font-medium">Powered by gasket-rs Web Gateway</span>
            <button @click="sendOnEnter = !sendOnEnter" class="hover:text-slate-300 transition-colors"
              :title="sendOnEnter ? 'Click to switch to Cmd+Enter to send' : 'Click to switch to Enter to send'">
              {{ sendOnEnter ? `${isMac ? 'Shift' : 'Shift'}+Enter for new line` : `Enter for new line` }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style>
.custom-scrollbar::-webkit-scrollbar { width: 6px; }
.custom-scrollbar::-webkit-scrollbar-track { background: rgba(0,0,0,0.1); border-radius: 4px; }
.custom-scrollbar::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.2); border-radius: 4px; }
.custom-scrollbar::-webkit-scrollbar-thumb:hover { background: rgba(255,255,255,0.3); }
</style>
