<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted, nextTick, computed } from 'vue';
import { Send, Cpu, Loader2, ChevronDown, ChevronRight, Check, Trash2, Wifi, WifiOff, Upload, Brain } from 'lucide-vue-next';
import { marked } from 'marked';
import type { Message } from '../App.vue';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Button } from '@/components/ui/button';

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

const emit = defineEmits<{
  (e: 'update-messages', messages: Message[]): void;
  (e: 'clear-messages'): void;
}>();

// Refs
const scrollAreaRef = ref<InstanceType<typeof ScrollArea> | null>(null);
const inputRef = ref<HTMLTextAreaElement | null>(null);
const inputValue = ref('');

// Local state
const localMessages = ref<Message[]>([]);
const isConnected = ref(false);
const isThinking = ref(false);
const isSending = ref(false);
const isReceiving = ref(false);
const ws = ref<WebSocket | null>(null);

// UI State
const expandedTools = ref<Record<string, boolean>>({});
const expandedThinking = ref<Record<string, boolean>>({});

// Computed status
type SessionStatus = 'disconnected' | 'idle' | 'sending' | 'receiving' | 'complete';
const sessionStatus = computed<SessionStatus>(() => {
  if (!isConnected.value) return 'disconnected';
  if (isSending.value) return 'sending';
  if (isReceiving.value || isThinking.value) return 'receiving';
  return 'idle';
});

const statusConfig = computed(() => {
  const configs: Record<SessionStatus, { text: string; color: string; bgColor: string; icon: any; animate: boolean }> = {
    disconnected: {
      text: 'Disconnected',
      color: 'text-red-400',
      bgColor: 'bg-red-500',
      icon: WifiOff,
      animate: false
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
    },
    complete: {
      text: 'Response complete',
      color: 'text-emerald-400',
      bgColor: 'bg-emerald-500',
      icon: Check,
      animate: false
    }
  };
  return configs[sessionStatus.value];
});

// Initialize
onMounted(() => {
  localMessages.value = JSON.parse(JSON.stringify(props.messages));
  connect();
  nextTick(() => {
    scrollToBottom();
  });
});

onUnmounted(() => {
  if (ws.value) {
    ws.value.close();
  }
});

// Reconnect string changed
watch(() => props.sessionId, () => {
  localMessages.value = JSON.parse(JSON.stringify(props.messages));
  connect();
  nextTick(() => {
    scrollToBottom();
  });
});

// Watch for messages changes (from parent) and scroll to bottom
watch(() => props.messages, (newMessages) => {
  localMessages.value = JSON.parse(JSON.stringify(newMessages));
}, { deep: true });

const connect = () => {
  if (ws.value) {
    ws.value.close();
  }

  const wsUrl = `ws://localhost:3000/ws?user_id=${encodeURIComponent(props.sessionId)}`;
  ws.value = new WebSocket(wsUrl);

  ws.value.onopen = () => {
    isConnected.value = true;
  };

  ws.value.onmessage = (event) => {
    handleMessage(event.data);
  };

  ws.value.onclose = () => {
    isConnected.value = false;
    isThinking.value = false;
  };

  ws.value.onerror = () => {
    isConnected.value = false;
    isThinking.value = false;
  };
};

// WebSocket Handlers
const getLatestBotMessage = () => {
  const lastMsg = localMessages.value[localMessages.value.length - 1];
  if (lastMsg && lastMsg.role === 'bot') {
    return lastMsg;
  }

  const newBotMsg: Message = {
    id: Date.now().toString(),
    role: 'bot',
    content: '',
    timestamp: Date.now()
  };
  localMessages.value.push(newBotMsg);
  return newBotMsg;
};

const handleMessage = (data: string) => {
  try {
    const msg = JSON.parse(data);
    const botMsg = getLatestBotMessage();

    isSending.value = false;
    isReceiving.value = true;

    switch (msg.type) {
      case 'thinking':
        isThinking.value = true;
        botMsg.thinking = (botMsg.thinking || '') + msg.content;
        break;
      case 'tool_start':
        isThinking.value = true;
        if (!botMsg.toolCalls) botMsg.toolCalls = [];
        botMsg.toolCalls.push({
          id: Date.now().toString(),
          name: msg.name,
          arguments: msg.arguments || '',
          status: 'running',
          result: null
        });
        break;
      case 'tool_end':
        isThinking.value = true;
        if (botMsg.toolCalls && botMsg.toolCalls.length > 0) {
          const activeTool = botMsg.toolCalls[botMsg.toolCalls.length - 1];
          activeTool.status = 'complete';
          activeTool.result = msg.output;
        }
        break;
      case 'content':
        isThinking.value = true;
        botMsg.content += msg.content;
        break;
      case 'done':
        isThinking.value = false;
        isReceiving.value = false;
        if (botMsg.content || (botMsg.toolCalls && botMsg.toolCalls.length > 0)) {
          expandedThinking.value[botMsg.id] = false;
        }
        // Expands full height and scroll to end of chat area
        setTimeout(() => {
          const scrollEl = getScrollElement(scrollAreaRef.value);
          if (scrollEl) {
            scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: 'smooth' });
          }
        }, 150);
        break;
      case 'text':
        isThinking.value = false;
        botMsg.content += msg.content;
        break;
    }
  } catch (e) {
    isThinking.value = false;
    isSending.value = false;
    const botMsg = getLatestBotMessage();
    botMsg.content += data;
  }

  emitMessages();
  scrollToBottom();
};

const emitMessages = () => {
  emit('update-messages', JSON.parse(JSON.stringify(localMessages.value)));
};

const sendMessage = () => {
  if (!inputValue.value.trim() || !isConnected.value || isSending.value || isReceiving.value) return;

  const text = inputValue.value;
  inputValue.value = '';

  const newMessageId = Date.now().toString();
  localMessages.value.push({
    id: newMessageId,
    role: 'user',
    content: text,
    timestamp: Date.now()
  });

  emitMessages();
  isSending.value = true;

  if (ws.value?.readyState === WebSocket.OPEN) {
    ws.value.send(text);
    setTimeout(() => {
      isSending.value = false;
      isReceiving.value = true;
    }, 100);
  }

  scrollToMessage(newMessageId);

  nextTick(() => {
    inputRef.value?.focus();
    if (inputRef.value) {
      inputRef.value.style.height = 'auto';
    }
  });
};

const autoResize = () => {
  const el = inputRef.value;
  if (!el) return;
  el.style.height = 'auto';
  el.style.height = `${el.scrollHeight}px`;
};

const scrollToBottom = async () => {
  if (isReceiving.value) {
    // Only auto scroll to bottom if we are receiving a response and already near the bottom
    // We don't want to auto-scroll if the user has scrolled up to read something else
    await nextTick();
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (!scrollEl) return;
    
    // Only scroll to bottom if we are relatively close to it already
    const isNearBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 150;
    if (isNearBottom) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  } else {
    // Forced scroll to bottom
    await nextTick();
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (scrollEl) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  }
};

const scrollToMessage = async (msgId: string) => {
  await nextTick();
  // With large messages, Vue needs a moment to mount the DOM element.
  // Using 150ms to ensure element exists before scrolling.
  setTimeout(() => {
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (!scrollEl) return;
    
    // Find the message element by id
    const messageEl = document.getElementById(`msg-${msgId}`);
    if (messageEl) {
      // Calculate position to scroll to
      const topPos = messageEl.offsetTop - 32;
      
      scrollEl.scrollTo({
        top: topPos,
        behavior: 'smooth'
      });
    }
  }, 150);
};

const renderMarkdown = (text: string) => {
  if (!text) return '';
  return marked(text, { breaks: true, gfm: true });
};

const clearHistory = () => {
  localMessages.value = [];
  emit('clear-messages');
};

const toggleToolExpand = (key: string) => {
  expandedTools.value[key] = !expandedTools.value[key];
};

const toggleThinkingExpand = (msgId: string) => {
  expandedThinking.value[msgId] = expandedThinking.value[msgId] === false ? true : false;
};
</script>

<template>
  <div class="flex h-full w-full relative">
    <!-- Main Chat Area -->
    <div class="flex flex-col flex-1 min-w-0">
      <!-- Header -->
      <header class="py-4 px-6 bg-slate-800/80 border-b border-white/10 flex justify-between items-center">
        <Button
          variant="ghost"
          size="sm"
          @click="clearHistory"
          :disabled="localMessages.length === 0"
          class="text-slate-400 hover:text-red-400 hover:bg-red-500/10 disabled:opacity-30"
        >
          <Trash2 class="w-4 h-4 mr-2" />
          Clear History
        </Button>
        <div class="flex items-center gap-3">
          <div
            class="flex items-center bg-black/30 px-3 py-1.5 rounded-full border border-white/5 transition-all"
            :class="{ 'animate-pulse': statusConfig.animate }"
          >
            <div
              class="w-2 h-2 rounded-full mr-2 transition-all"
              :class="[statusConfig.bgColor, { 'animate-ping': statusConfig.animate }]"
              style="box-shadow: 0 0 8px currentColor;"
            ></div>
            <component
              :is="statusConfig.icon"
              class="w-3.5 h-3.5 mr-1.5"
              :class="[statusConfig.color, { 'animate-spin': statusConfig.animate && sessionStatus === 'receiving' }]"
            />
            <span class="text-xs font-medium" :class="statusConfig.color">
              {{ statusConfig.text }}
            </span>
          </div>
        </div>
      </header>

      <!-- Messages -->
      <ScrollArea class="flex-1 p-6" ref="scrollAreaRef">
        <div class="flex flex-col gap-6 max-w-4xl mx-auto w-full pb-4">
          <div
            v-for="msg in localMessages"
            :key="msg.id"
            :id="`msg-${msg.id}`"
            class="flex flex-col animate-in fade-in slide-in-from-bottom-2 duration-300 w-full"
            :class="msg.role === 'user' ? 'self-end max-w-[85%]' : (msg.role === 'system' ? 'self-center max-w-[95%]' : 'self-start w-full')"
          >
            <div v-if="msg.role !== 'system'" class="text-xs text-slate-400 mb-1.5" :class="msg.role === 'user' ? 'text-right mx-1' : 'ml-0 font-semibold text-slate-300'">
              {{ msg.role === 'user' ? 'You' : 'Nanobot' }}
            </div>

            <div class="relative break-words transition-all" :class="{
              'rounded-2xl bg-gradient-to-br from-blue-500 to-blue-700 text-white p-3 px-4 rounded-br-sm shadow-lg shadow-blue-500/20': msg.role === 'user',
              'w-full py-1 text-slate-200': msg.role === 'bot',
              'rounded-xl bg-black/20 text-slate-400 py-1.5 px-3 text-xs text-center mx-auto': msg.role === 'system'
            }">
              <!-- System message -->
              <div v-if="msg.role === 'system'" class="text-xs">
                {{ msg.content }}
              </div>

              <!-- Bot message structure -->
              <template v-else-if="msg.role === 'bot'">
                <!-- Thinking section -->
                <div v-if="msg.thinking" class="mb-4 pb-4 border-b border-white/10">
                  <div
                    class="flex items-center gap-2 mb-2 cursor-pointer select-none hover:bg-slate-700/30 p-1.5 -ml-1.5 rounded transition-colors"
                    @click="toggleThinkingExpand(msg.id)"
                  >
                    <Brain class="w-4 h-4 text-violet-400" />
                    <span class="text-xs font-medium text-violet-400">Thinking Process</span>
                    <ChevronDown v-if="expandedThinking[msg.id] !== false" class="w-4 h-4 text-slate-500 ml-auto" />
                    <ChevronRight v-else class="w-4 h-4 text-slate-500 ml-auto" />
                  </div>
                  <div
                    v-show="expandedThinking[msg.id] !== false"
                    class="bg-slate-900/50 rounded-lg p-3 border border-slate-700/50"
                  >
                    <div class="text-[13px] text-slate-400/80 italic whitespace-pre-wrap leading-relaxed">
                      {{ msg.thinking }}
                    </div>
                  </div>
                </div>

                <!-- Tool calls section - highly compact -->
                <div
                  v-if="msg.toolCalls && msg.toolCalls.length > 0"
                  class="mb-3 w-full"
                >
                  <div class="flex flex-wrap gap-2 w-full">
                    <div
                      v-for="(tool, index) in msg.toolCalls"
                      :key="index"
                      class="flex flex-col border border-slate-700/60 bg-slate-900/60 rounded-md overflow-hidden text-sm"
                      :class="expandedTools[msg.id + '_' + index] ? 'w-full' : 'w-auto max-w-full'"
                    >
                      <!-- Tool header - clickable to expand -->
                      <div
                        class="flex items-center gap-2 p-1.5 px-2.5 cursor-pointer hover:bg-slate-700/40 transition-colors select-none"
                        @click="toggleToolExpand(msg.id + '_' + index)"
                      >
                        <Loader2 v-if="tool.status === 'running'" class="animate-spin w-3.5 h-3.5 text-emerald-400 shrink-0" />
                        <Check v-else class="w-3.5 h-3.5 text-emerald-400 shrink-0" />
                        <span class="font-mono text-xs text-slate-300 truncate"><span class="text-slate-500">Call </span>{{ tool.name }}</span>
                        <ChevronDown v-if="expandedTools[msg.id + '_' + index]" class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                        <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                      </div>
                      
                      <!-- Tool details - only shown when expanded -->
                      <div v-if="expandedTools[msg.id + '_' + index]" class="p-2 pt-0 border-t border-slate-700/60 bg-slate-950/50">
                        <div class="text-[11px] font-semibold text-slate-500 mb-1 mt-2 uppercase tracking-wider">Arguments</div>
                        <pre class="bg-black/60 rounded p-1.5 font-mono text-[11px] text-slate-300 overflow-x-auto whitespace-pre-wrap break-all border-l-2 border-blue-500/50 custom-scrollbar m-0"><code>{{ tool.arguments || '{}' }}</code></pre>

                        <template v-if="tool.result">
                          <div class="text-[11px] font-semibold text-slate-500 mb-1 mt-2 uppercase tracking-wider">Result</div>
                          <pre class="bg-black/60 rounded p-1.5 font-mono text-[11px] text-slate-300 overflow-x-auto whitespace-pre-wrap break-all border-l-2 border-amber-500/50 m-0"><code>{{ tool.result }}</code></pre>
                        </template>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- Final response content - separate from tool calls -->
                <div
                  v-if="msg.content"
                  class="prose prose-invert max-w-none text-sm leading-relaxed transition-all"
                  v-html="renderMarkdown(msg.content)"
                ></div>

                <!-- Typing indicator when no content yet -->
                <div v-else-if="localMessages[localMessages.length - 1].id === msg.id && (isReceiving || isThinking)" class="flex items-center gap-2 text-slate-400">
                  <div class="flex gap-1">
                    <span class="w-1.5 h-1.5 bg-blue-500 rounded-full animate-bounce [animation-delay:0s]"></span>
                    <span class="w-1.5 h-1.5 bg-blue-500 rounded-full animate-bounce [animation-delay:-0.15s]"></span>
                    <span class="w-1.5 h-1.5 bg-blue-500 rounded-full animate-bounce [animation-delay:-0.3s]"></span>
                  </div>
                  <span class="text-xs">Generating response...</span>
                </div>
              </template>

              <!-- User message -->
              <div v-else class="text-sm">
                {{ msg.content }}
              </div>
            </div>
          </div>
        </div>
      </ScrollArea>

      <!-- Input Area -->
      <div class="p-6 pt-0 bg-transparent shrink-0">
        <div class="max-w-4xl mx-auto w-full relative">
          <div
            class="flex items-end bg-slate-900/70 border border-white/10 rounded-2xl p-2 shadow-xl backdrop-blur-xl transition-all"
            :class="{
              'focus-within:border-blue-500/50 focus-within:ring-2 focus-within:ring-blue-500/20': sessionStatus === 'idle' || sessionStatus === 'disconnected',
              'border-violet-500/30 ring-2 ring-violet-500/20': sessionStatus === 'receiving',
              'border-blue-500/30 ring-2 ring-blue-500/20': sessionStatus === 'sending'
            }"
          >
            <textarea
              ref="inputRef"
              v-model="inputValue"
              @keydown.enter.meta.prevent="sendMessage"
              @keydown.enter.ctrl.prevent="sendMessage"
              @input="autoResize"
              :placeholder="sessionStatus === 'receiving' ? 'AI is processing your request...' : sessionStatus === 'sending' ? 'Sending your message...' : (localMessages.length > 0 ? 'Ready for your next prompt... (Cmd+Enter to send)' : 'Type your message... (Cmd+Enter to send)')"
              :disabled="!isConnected || sessionStatus === 'receiving' || sessionStatus === 'sending'"
              autofocus
              rows="1"
              class="flex-1 overflow-x-hidden border-0 bg-transparent shadow-none focus:outline-none focus:ring-0 text-slate-100 px-3 py-2.5 disabled:opacity-50 disabled:cursor-not-allowed resize-none custom-scrollbar min-h-[44px] max-h-[200px]"
            ></textarea>
            <Button
              @click="sendMessage"
              :disabled="!inputValue.trim() || !isConnected || sessionStatus === 'receiving' || sessionStatus === 'sending'"
              class="w-11 h-11 rounded-xl text-white shrink-0 ml-2 transition-all"
              :class="{
                'bg-blue-500 hover:bg-blue-400': sessionStatus === 'idle',
                'bg-slate-600 cursor-not-allowed': sessionStatus !== 'idle'
              }"
              size="icon"
            >
              <Send v-if="sessionStatus === 'idle'" class="w-5 h-5" />
              <Loader2 v-else class="w-5 h-5 animate-spin" />
            </Button>
          </div>
          <div class="text-center text-xs text-slate-500 mt-3 font-medium">
            Powered by Nanobot-rs Web Gateway
          </div>
        </div>
      </div>
    </div>

  </div>
</template>

<style>
/* Markdown specific styling */
.prose p { margin-bottom: 0.75em; }
.prose p:last-child { margin-bottom: 0; }
.prose a { color: #60a5fa; text-decoration: none; }
.prose a:hover { text-decoration: underline; }
.prose code { background-color: rgba(0,0,0,0.3); padding: 0.2em 0.4em; border-radius: 4px; font-family: monospace; font-size: 0.9em; color: #e2e8f0; }
.prose pre { background-color: rgba(0,0,0,0.4); padding: 12px; border-radius: 8px; overflow-x: auto; margin: 0.75em 0; border: 1px solid rgba(255,255,255,0.1); }
.prose pre code { background-color: transparent; padding: 0; font-size: 0.9em; }

/* Custom Scrollbar for thinking panel */
.custom-scrollbar::-webkit-scrollbar {
  width: 6px;
}
.custom-scrollbar::-webkit-scrollbar-track {
  background: rgba(0,0,0,0.1);
  border-radius: 4px;
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background: rgba(255,255,255,0.2);
  border-radius: 4px;
}
.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background: rgba(255,255,255,0.3);
}
</style>
