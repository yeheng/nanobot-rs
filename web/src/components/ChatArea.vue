<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted, nextTick, computed } from 'vue';
import { Send, Cpu, Loader2, ChevronDown, ChevronRight, Check, Trash2, Wifi, WifiOff, Upload, Brain, Copy, CheckCheck, Square, ArrowDown, Sparkles, AlertCircle, X as XIcon, RotateCcw } from 'lucide-vue-next';
import { Marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import hljs from 'highlight.js';
import DOMPurify from 'dompurify';
import mermaid from 'mermaid';
import type { Message } from '../App.vue';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Button } from '@/components/ui/button';

// Initialize mermaid
let mermaidIdCounter = 0;
mermaid.initialize({
  startOnLoad: false,
  theme: 'dark',
  themeVariables: {
    darkMode: true,
    background: '#1e293b',
    primaryColor: '#3b82f6',
    primaryTextColor: '#e2e8f0',
    primaryBorderColor: '#475569',
    lineColor: '#94a3b8',
    secondaryColor: '#8b5cf6',
    tertiaryColor: '#1e293b',
    fontFamily: 'Inter, sans-serif',
    fontSize: '14px',
  },
  securityLevel: 'loose',
});

// Configure marked with highlight.js + mermaid code block handling
const mermaidRenderer = {
  code({ text, lang }: { text: string; lang?: string }) {
    if (lang === 'mermaid') {
      const id = `mermaid-${Date.now()}-${mermaidIdCounter++}`;
      return `<div class="mermaid-container"><pre class="mermaid" id="${id}">${text}</pre></div>`;
    }
    return false; // fall through to default renderer
  }
};

const markedInstance = new Marked(
  markedHighlight({
    langPrefix: 'hljs language-',
    highlight(code: string, lang: string) {
      if (lang === 'mermaid') return code; // skip hljs for mermaid
      if (lang && hljs.getLanguage(lang)) {
        try {
          return hljs.highlight(code, { language: lang }).value;
        } catch (__) { /* ignore */ }
      }
      try {
        return hljs.highlightAuto(code).value;
      } catch (__) { /* ignore */ }
      return code;
    }
  })
);
markedInstance.use({ renderer: mermaidRenderer });
markedInstance.setOptions({ breaks: true, gfm: true });

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

// Reconnection state
const reconnectAttempts = ref(0);
const maxReconnectAttempts = 5;
const showReconnectButton = ref(false);
const isReconnecting = ref(false);
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

// UI State
const expandedTools = ref<Record<string, boolean>>({});
const expandedThinking = ref<Record<string, boolean>>({});
const copiedMessageId = ref<string | null>(null);
const copiedCodeBlock = ref<string | null>(null);
const showScrollButton = ref(false);
const sendOnEnter = ref(true); // Enter sends, Shift+Enter newline

// Tool timing
const toolStartTimes = ref<Record<string, number>>({});

// Error messages
const errorBanner = ref<string | null>(null);
let errorBannerTimer: ReturnType<typeof setTimeout> | null = null;

// Suggested prompts for empty state
const suggestedPrompts = [
  { icon: '💡', text: 'Explain how this project is structured' },
  { icon: '🔍', text: 'Help me find and fix bugs in my code' },
  { icon: '📝', text: 'Write a unit test for a function' },
  { icon: '🚀', text: 'Suggest performance improvements' },
];

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
      text: isReconnecting.value ? `Reconnecting (${reconnectAttempts.value}/${maxReconnectAttempts})...` : 'Disconnected',
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

// Check if user has any real messages (not system)
const hasUserMessages = computed(() => {
  return localMessages.value.some(m => m.role === 'user' || m.role === 'bot');
});

// Completed tool count for a message
const getToolProgress = (toolCalls: any[]) => {
  const completed = toolCalls.filter(t => t.status === 'complete' || t.status === 'error').length;
  return { completed, total: toolCalls.length };
};

const toggleThinkingExpand = (id: string) => {
  if (expandedThinking.value[id] === undefined) {
    expandedThinking.value[id] = false;
  } else {
    expandedThinking.value[id] = !expandedThinking.value[id];
  }
};

// Platform detection
const isMac = computed(() => navigator.platform.toUpperCase().indexOf('MAC') >= 0);
const sendShortcut = computed(() => {
  if (sendOnEnter.value) return 'Enter';
  return isMac.value ? '⌘+Enter' : 'Ctrl+Enter';
});

// Initialize
onMounted(() => {
  localMessages.value = JSON.parse(JSON.stringify(props.messages));
  connect();
  nextTick(() => {
    scrollToBottom();
    renderMermaidDiagrams();
  });

  // Setup scroll button observer
  setupScrollObserver();
});

onUnmounted(() => {
  if (ws.value) {
    ws.value.close();
  }
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
  }
  if (errorBannerTimer) {
    clearTimeout(errorBannerTimer);
  }
});

// Reconnect string changed
watch(() => props.sessionId, () => {
  localMessages.value = JSON.parse(JSON.stringify(props.messages));
  reconnectAttempts.value = 0;
  showReconnectButton.value = false;
  connect();
  nextTick(() => {
    scrollToBottom();
    renderMermaidDiagrams();
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
    reconnectAttempts.value = 0;
    showReconnectButton.value = false;
    isReconnecting.value = false;
  };

  ws.value.onmessage = (event) => {
    handleMessage(event.data);
  };

  ws.value.onclose = () => {
    isConnected.value = false;
    isThinking.value = false;
    isReceiving.value = false;
    isSending.value = false;
    attemptReconnect();
  };

  ws.value.onerror = () => {
    isConnected.value = false;
    isThinking.value = false;
  };
};

// Auto-reconnect with exponential backoff
const attemptReconnect = () => {
  if (reconnectAttempts.value >= maxReconnectAttempts) {
    showReconnectButton.value = true;
    isReconnecting.value = false;
    return;
  }

  isReconnecting.value = true;
  const delay = Math.min(1000 * Math.pow(2, reconnectAttempts.value), 30000);
  reconnectAttempts.value++;

  reconnectTimer = setTimeout(() => {
    connect();
  }, delay);
};

const manualReconnect = () => {
  reconnectAttempts.value = 0;
  showReconnectButton.value = false;
  isReconnecting.value = true;
  connect();
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
        
        if (!botMsg.steps) botMsg.steps = [];
        let lastStep = botMsg.steps[botMsg.steps.length - 1];
        if (!lastStep || lastStep.type !== 'thinking') {
          const stepId = 'think_' + Date.now() + '_' + Math.random().toString(36).substr(2,9);
          botMsg.steps.push({
            id: stepId,
            type: 'thinking',
            content: msg.content
          });
        } else {
          lastStep.content += msg.content;
        }
        break;
      case 'tool_start':
        isThinking.value = true;
        if (!botMsg.toolCalls) botMsg.toolCalls = [];
        if (!botMsg.steps) botMsg.steps = [];
        let lastToolGroup = botMsg.steps[botMsg.steps.length - 1];
        if (!lastToolGroup || lastToolGroup.type !== 'tool_group') {
           lastToolGroup = {
             id: 'tool_group_' + Date.now() + '_' + Math.random().toString(36).substr(2,9),
             type: 'tool_group',
             tools: []
           };
           botMsg.steps.push(lastToolGroup);
        }

        const toolId = Date.now().toString() + '_' + Math.random().toString(36).substr(2,9);
        const newTool = {
          id: toolId,
          name: msg.name,
          arguments: msg.arguments || '',
          status: 'running',
          result: null
        };
        
        botMsg.toolCalls.push(newTool);
        lastToolGroup.tools.push(newTool);
        
        // Record start time
        toolStartTimes.value[toolId] = Date.now();
        break;
      case 'tool_end':
        isThinking.value = true;
        if (botMsg.toolCalls && botMsg.toolCalls.length > 0) {
          // Match by tool name (msg.name) instead of blindly using the last entry.
          // Find the last tool with matching name that is still 'running'.
          // This correctly handles: tool_start(A), tool_start(B), tool_end(A), tool_end(B)
          const matchingTool = [...botMsg.toolCalls].reverse().find(
            t => t.name === msg.name && t.status === 'running'
          );
          const activeTool = matchingTool
            || [...botMsg.toolCalls].reverse().find(t => t.status === 'running')
            || botMsg.toolCalls[botMsg.toolCalls.length - 1];
          activeTool.status = msg.error ? 'error' : 'complete';
          activeTool.result = msg.error || msg.output;
          // Calculate duration
          if (toolStartTimes.value[activeTool.id]) {
            activeTool.duration = ((Date.now() - toolStartTimes.value[activeTool.id]) / 1000).toFixed(1);
            delete toolStartTimes.value[activeTool.id];
          }
        }
        break;
      case 'content':
      case 'text':
        isThinking.value = msg.type === 'content';
        botMsg.content += msg.content;
        
        if (!botMsg.steps) botMsg.steps = [];
        let lastTextStep = botMsg.steps[botMsg.steps.length - 1];
        if (!lastTextStep || lastTextStep.type !== 'content') {
           const stepId = 'content_' + Date.now() + '_' + Math.random().toString(36).substr(2,9);
           botMsg.steps.push({
             id: stepId,
             type: 'content',
             content: msg.content
           });
        } else {
           lastTextStep.content += msg.content;
        }
        break;
      case 'error':
        // Handle error messages from backend
        isThinking.value = false;
        showError(msg.content || msg.message || 'An error occurred');
        break;
      case 'done':
        isThinking.value = false;
        isReceiving.value = false;
        // Scroll to end
        setTimeout(() => {
          const scrollEl = getScrollElement(scrollAreaRef.value);
          if (scrollEl) {
            scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: 'smooth' });
          }
        }, 150);
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

const showError = (message: string) => {
  errorBanner.value = message;
  if (errorBannerTimer) clearTimeout(errorBannerTimer);
  errorBannerTimer = setTimeout(() => {
    errorBanner.value = null;
  }, 8000);
};

const dismissError = () => {
  errorBanner.value = null;
  if (errorBannerTimer) clearTimeout(errorBannerTimer);
};

// Debounced emit for localStorage performance
let emitTimer: ReturnType<typeof setTimeout> | null = null;
const emitMessages = () => {
  if (emitTimer) clearTimeout(emitTimer);
  emitTimer = setTimeout(() => {
    emit('update-messages', JSON.parse(JSON.stringify(localMessages.value)));
  }, 500);
};

// Force immediate emit (for send message)
const emitMessagesNow = () => {
  if (emitTimer) clearTimeout(emitTimer);
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

  emitMessagesNow();
  isSending.value = true;

  if (ws.value?.readyState === WebSocket.OPEN) {
    ws.value.send(text);
    // State transition from 'sending' → 'receiving' is handled by
    // handleMessage() when the first server response arrives.
    // No artificial setTimeout — it races with fast responses and
    // can override the 'done' state back to 'receiving'.
  }

  scrollToMessage(newMessageId);

  nextTick(() => {
    inputRef.value?.focus();
    if (inputRef.value) {
      inputRef.value.style.height = 'auto';
    }
  });
};

// Stop generating / cancel
const stopGenerating = () => {
  if (ws.value?.readyState === WebSocket.OPEN) {
    ws.value.send(JSON.stringify({ type: 'cancel' }));
  }
  isThinking.value = false;
  isReceiving.value = false;
  isSending.value = false;

  // Mark the last bot message as done
  const lastMsg = localMessages.value[localMessages.value.length - 1];
  if (lastMsg && lastMsg.role === 'bot') {
    if (lastMsg.toolCalls) {
      lastMsg.toolCalls.forEach(tc => {
        if (tc.status === 'running') tc.status = 'error';
      });
    }
    if (lastMsg.steps) {
      lastMsg.steps.forEach((step: any) => {
        if (step.type === 'tool_group' && step.tools) {
          step.tools.forEach((tc: any) => {
            if (tc.status === 'running') tc.status = 'error';
          });
        }
      });
    }
  }
  emitMessagesNow();
};

// Send via suggested prompt
const sendPrompt = (prompt: string) => {
  inputValue.value = prompt;
  sendMessage();
};

const handleKeydown = (event: KeyboardEvent) => {
  if (sendOnEnter.value) {
    // Enter sends, Shift+Enter adds newline
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      sendMessage();
    }
  } else {
    // Cmd/Ctrl+Enter sends
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

// Scroll management
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
  if (isReceiving.value) {
    await nextTick();
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (!scrollEl) return;
    
    const isNearBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 150;
    if (isNearBottom) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  } else {
    await nextTick();
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (scrollEl) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  }
};

const forceScrollToBottom = () => {
  const scrollEl = getScrollElement(scrollAreaRef.value);
  if (scrollEl) {
    scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: 'smooth' });
  }
};

const scrollToMessage = async (msgId: string) => {
  await nextTick();
  setTimeout(() => {
    const scrollEl = getScrollElement(scrollAreaRef.value);
    if (!scrollEl) return;
    
    const messageEl = document.getElementById(`msg-${msgId}`);
    if (messageEl) {
      const topPos = messageEl.offsetTop - 32;
      scrollEl.scrollTo({
        top: topPos,
        behavior: 'smooth'
      });
    }
  }, 150);
};

// Rendering
const renderMarkdown = (text: string) => {
  if (!text) return '';
  const rawHtml = markedInstance.parse(text) as string;
  // Allow mermaid class and id attributes through DOMPurify
  return DOMPurify.sanitize(rawHtml, {
    ADD_TAGS: ['pre'],
    ADD_ATTR: ['class', 'id'],
  });
};

// Render mermaid diagrams after DOM updates
const renderMermaidDiagrams = async () => {
  await nextTick();
  try {
    const elements = document.querySelectorAll('pre.mermaid:not([data-processed])');
    if (elements.length > 0) {
      // Clear data-processed to allow re-rendering if needed, 
      // but usually we just want to render new ones.
      await mermaid.run({ nodes: elements as any });
    }
  } catch (e) {
    console.warn('Mermaid rendering error:', e);
  }
};

// Watch for message changes - only render if not currently receiving
watch(localMessages, () => {
  if (!isReceiving.value && !isThinking.value) {
    renderMermaidDiagrams();
  }
}, { deep: true });

// Watch for completion of response to trigger rendering
watch([isReceiving, isThinking], ([newReceiving, newThinking]) => {
  if (!newReceiving && !newThinking) {
    renderMermaidDiagrams();
  }
});

// Copy message content
const copyMessage = async (msg: Message) => {
  try {
    await navigator.clipboard.writeText(msg.content);
    copiedMessageId.value = msg.id;
    setTimeout(() => {
      copiedMessageId.value = null;
    }, 2000);
  } catch (e) {
    console.error('Failed to copy message:', e);
  }
};

// Copy code block
const copyCodeBlock = async (event: MouseEvent) => {
  const button = (event.target as HTMLElement).closest('.code-copy-btn');
  if (!button) return;
  const pre = button.closest('.code-block-wrapper')?.querySelector('pre');
  if (!pre) return;
  const code = pre.textContent || '';
  
  try {
    await navigator.clipboard.writeText(code);
    const blockId = button.getAttribute('data-block-id') || '';
    copiedCodeBlock.value = blockId;
    setTimeout(() => {
      copiedCodeBlock.value = null;
    }, 2000);
  } catch (e) {
    console.error('Failed to copy code:', e);
  }
};

// Format timestamp
const formatTime = (timestamp: number) => {
  return new Date(timestamp).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
};

const clearHistory = () => {
  localMessages.value = [];
  emit('clear-messages');
};

const toggleToolExpand = (key: string) => {
  expandedTools.value[key] = !expandedTools.value[key];
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
          <!-- Reconnect button -->
          <Button
            v-if="showReconnectButton"
            variant="outline"
            size="sm"
            @click="manualReconnect"
            class="text-amber-400 border-amber-500/30 hover:bg-amber-500/10 text-xs"
          >
            <RotateCcw class="w-3.5 h-3.5 mr-1.5" />
            Reconnect
          </Button>
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
              :class="[statusConfig.color, { 'animate-spin': statusConfig.animate && (sessionStatus === 'receiving' || isReconnecting) }]"
            />
            <span class="text-xs font-medium" :class="statusConfig.color">
              {{ statusConfig.text }}
            </span>
          </div>
        </div>
      </header>

      <!-- Error Banner -->
      <div
        v-if="errorBanner"
        class="mx-6 mt-3 flex items-center gap-2 bg-red-500/15 border border-red-500/30 text-red-300 px-4 py-2.5 rounded-lg text-sm animate-in fade-in slide-in-from-top-2 duration-300"
      >
        <AlertCircle class="w-4 h-4 shrink-0 text-red-400" />
        <span class="flex-1">{{ errorBanner }}</span>
        <button @click="dismissError" class="p-0.5 hover:bg-red-500/20 rounded transition-colors">
          <XIcon class="w-3.5 h-3.5" />
        </button>
      </div>

      <!-- Messages -->
      <ScrollArea class="flex-1 p-6" ref="scrollAreaRef">
        <!-- Empty State -->
        <div
          v-if="!hasUserMessages"
          class="flex flex-col items-center justify-center h-full max-w-2xl mx-auto text-center py-20"
        >
          <div class="w-16 h-16 rounded-2xl bg-gradient-to-br from-blue-500 to-violet-600 flex items-center justify-center mb-6 shadow-lg shadow-blue-500/20">
            <Sparkles class="w-8 h-8 text-white" />
          </div>
          <h2 class="text-2xl font-semibold text-slate-100 mb-2">How can I help you today?</h2>
          <p class="text-slate-400 mb-8 text-sm">Ask me anything about your code, project, or ideas.</p>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 w-full">
            <button
              v-for="(prompt, idx) in suggestedPrompts"
              :key="idx"
              @click="sendPrompt(prompt.text)"
              :disabled="!isConnected"
              class="flex items-center gap-3 p-4 bg-slate-800/60 hover:bg-slate-700/60 border border-white/5 hover:border-white/15 rounded-xl text-left text-sm text-slate-300 hover:text-slate-100 transition-all duration-200 disabled:opacity-40 disabled:cursor-not-allowed group"
            >
              <span class="text-lg flex-shrink-0 group-hover:scale-110 transition-transform">{{ prompt.icon }}</span>
              <span>{{ prompt.text }}</span>
            </button>
          </div>
        </div>

        <!-- Messages List -->
        <div v-else class="flex flex-col gap-6 max-w-4xl mx-auto w-full pb-4">
          <div
            v-for="msg in localMessages"
            :key="msg.id"
            :id="`msg-${msg.id}`"
            class="flex flex-col animate-in fade-in slide-in-from-bottom-2 duration-300 w-full group/msg"
            :class="msg.role === 'user' ? 'self-end max-w-[85%]' : (msg.role === 'system' ? 'self-center max-w-[95%]' : 'self-start w-full')"
          >
            <div v-if="msg.role !== 'system'" class="flex items-center gap-2 mb-1.5" :class="msg.role === 'user' ? 'justify-end mx-1' : 'ml-0'">
              <span class="text-xs" :class="msg.role === 'user' ? 'text-slate-400' : 'font-semibold text-slate-300'">
                {{ msg.role === 'user' ? 'You' : 'Nanobot' }}
              </span>
              <span class="text-[10px] text-slate-500" :title="new Date(msg.timestamp).toLocaleString()">
                {{ formatTime(msg.timestamp) }}
              </span>
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
                <!-- Steps section (interleaved thinking and tools) -->
                <template v-if="msg.steps && msg.steps.length > 0">
                  <div v-for="(step, stepIdx) in msg.steps" :key="step.id" class="mb-4">
                    <!-- Thinking Step -->
                    <div v-if="step.type === 'thinking'" class="pb-4" :class="{ 'border-b border-white/10': stepIdx < msg.steps.length - 1 || msg.content }">
                      <div
                        class="flex items-center gap-2 mb-2 cursor-pointer select-none hover:bg-slate-700/30 p-1.5 -ml-1.5 rounded transition-colors"
                        @click="toggleThinkingExpand(step.id)"
                      >
                        <Brain class="w-4 h-4 text-violet-400" :class="{ 'animate-pulse': isThinking && localMessages[localMessages.length - 1]?.id === msg.id && stepIdx === msg.steps.length - 1 }" />
                        <span class="text-xs font-medium text-violet-400">
                          {{ isThinking && localMessages[localMessages.length - 1]?.id === msg.id && stepIdx === msg.steps.length - 1 ? 'Thinking...' : 'Thinking Process' }}
                        </span>
                        <ChevronDown v-if="expandedThinking[step.id] !== false" class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                        <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                      </div>
                      <div
                        v-show="expandedThinking[step.id] !== false"
                        class="bg-slate-900/50 rounded-lg p-3 border border-slate-700/50"
                      >
                        <div class="text-[13px] text-slate-400/80 italic whitespace-pre-wrap leading-relaxed">
                          {{ step.content }}
                        </div>
                      </div>
                    </div>
                    
                    <!-- Tool Group Step -->
                    <div v-else-if="step.type === 'tool_group'" class="w-full">
                      <div v-if="step.tools.length > 1" class="text-[11px] text-slate-500 mb-1.5 font-medium">
                        {{ getToolProgress(step.tools).completed }}/{{ getToolProgress(step.tools).total }} tools completed
                      </div>
                      <div class="flex flex-wrap gap-2 w-full">
                        <div
                          v-for="(tool, index) in step.tools"
                          :key="tool.id"
                          class="flex flex-col border rounded-md overflow-hidden text-sm"
                          :class="{
                            'border-slate-700/60 bg-slate-900/60': tool.status !== 'error',
                            'border-red-500/40 bg-red-950/30': tool.status === 'error',
                            'w-full': expandedTools[msg.id + '_' + stepIdx + '_' + index],
                            'w-auto max-w-full': !expandedTools[msg.id + '_' + stepIdx + '_' + index]
                          }"
                        >
                          <!-- Tool header -->
                          <div
                            class="flex items-center gap-2 p-1.5 px-2.5 cursor-pointer hover:bg-slate-700/40 transition-colors select-none"
                            @click="toggleToolExpand(msg.id + '_' + stepIdx + '_' + index)"
                          >
                            <Loader2 v-if="tool.status === 'running'" class="animate-spin w-3.5 h-3.5 text-emerald-400 shrink-0" />
                            <Check v-else-if="tool.status === 'complete'" class="w-3.5 h-3.5 text-emerald-400 shrink-0" />
                            <AlertCircle v-else class="w-3.5 h-3.5 text-red-400 shrink-0" />
                            <span class="font-mono text-xs truncate" :class="tool.status === 'error' ? 'text-red-300' : 'text-slate-300'">
                              <span class="text-slate-500">Call </span>{{ tool.name }}
                            </span>
                            <span v-if="tool.duration" class="text-[10px] text-slate-500 ml-1 shrink-0">{{ tool.duration }}s</span>
                            <ChevronDown v-if="expandedTools[msg.id + '_' + stepIdx + '_' + index]" class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                            <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                          </div>
                          
                          <!-- Tool details -->
                          <div v-if="expandedTools[msg.id + '_' + stepIdx + '_' + index]" class="p-2 pt-0 border-t border-slate-700/60 bg-slate-950/50">
                            <div class="text-[11px] font-semibold text-slate-500 mb-1 mt-2 uppercase tracking-wider">Arguments</div>
                            <pre class="bg-black/60 rounded p-1.5 font-mono text-[11px] text-slate-300 overflow-x-auto whitespace-pre-wrap break-all border-l-2 border-blue-500/50 custom-scrollbar m-0"><code>{{ tool.arguments || '{}' }}</code></pre>

                            <template v-if="tool.result">
                              <div class="text-[11px] font-semibold mb-1 mt-2 uppercase tracking-wider" :class="tool.status === 'error' ? 'text-red-400' : 'text-slate-500'">
                                {{ tool.status === 'error' ? 'Error' : 'Result' }}
                              </div>
                              <pre class="bg-black/60 rounded p-1.5 font-mono text-[11px] overflow-x-auto whitespace-pre-wrap break-all border-l-2 m-0"
                                   :class="tool.status === 'error' ? 'text-red-300 border-red-500/50' : 'text-slate-300 border-amber-500/50'"><code>{{ tool.result }}</code></pre>
                            </template>
                          </div>
                        </div>
                      </div>
                    </div>
                    <!-- Text Content Step -->
                    <div v-else-if="step.type === 'content'" class="w-full mb-4">
                      <div
                        class="prose prose-invert max-w-none text-sm leading-relaxed transition-all relative group/content"
                        @click="copyCodeBlock"
                      >
                        <!-- Copy entire message button (shown on the last content step) -->
                        <button
                          v-if="stepIdx === msg.steps.length - 1 || !msg.steps.slice(stepIdx + 1).some((s: any) => s.type === 'content')"
                          @click.stop="copyMessage(msg)"
                          class="absolute -top-1 right-0 p-1.5 rounded-md bg-slate-800/80 border border-white/10 text-slate-400 hover:text-slate-200 hover:bg-slate-700/80 opacity-0 group-hover/msg:opacity-100 transition-all z-10"
                          title="Copy message"
                        >
                          <CheckCheck v-if="copiedMessageId === msg.id" class="w-3.5 h-3.5 text-emerald-400" />
                          <Copy v-else class="w-3.5 h-3.5" />
                        </button>
                        <div v-html="renderMarkdown(step.content)"></div>
                      </div>
                    </div>
                    
                  </div>
                </template>

                <!-- Legacy format fallback for old history messages -->
                <template v-else>
                  <div v-if="msg.thinking" class="mb-4 pb-4 border-b border-white/10">
                    <div
                      class="flex items-center gap-2 mb-2 cursor-pointer select-none hover:bg-slate-700/30 p-1.5 -ml-1.5 rounded transition-colors"
                      @click="toggleThinkingExpand(msg.id)"
                    >
                      <Brain class="w-4 h-4 text-violet-400" :class="{ 'animate-pulse': isThinking && localMessages[localMessages.length - 1]?.id === msg.id }" />
                      <span class="text-xs font-medium text-violet-400">
                        {{ isThinking && localMessages[localMessages.length - 1]?.id === msg.id ? 'Thinking...' : 'Thinking Process' }}
                      </span>
                      <ChevronDown v-if="expandedThinking[msg.id] !== false" class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                      <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
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
  
                  <!-- Tool calls section -->
                  <div
                    v-if="msg.toolCalls && msg.toolCalls.length > 0"
                    class="mb-3 w-full"
                  >
                    <!-- Tool progress indicator -->
                    <div v-if="msg.toolCalls.length > 1" class="text-[11px] text-slate-500 mb-1.5 font-medium">
                      {{ getToolProgress(msg.toolCalls).completed }}/{{ getToolProgress(msg.toolCalls).total }} tools completed
                    </div>
                    <div class="flex flex-wrap gap-2 w-full">
                      <div
                        v-for="(tool, index) in msg.toolCalls"
                        :key="index"
                        class="flex flex-col border rounded-md overflow-hidden text-sm"
                        :class="{
                          'border-slate-700/60 bg-slate-900/60': tool.status !== 'error',
                          'border-red-500/40 bg-red-950/30': tool.status === 'error',
                          'w-full': expandedTools[msg.id + '_' + index],
                          'w-auto max-w-full': !expandedTools[msg.id + '_' + index]
                        }"
                      >
                        <!-- Tool header -->
                        <div
                          class="flex items-center gap-2 p-1.5 px-2.5 cursor-pointer hover:bg-slate-700/40 transition-colors select-none"
                          @click="toggleToolExpand(msg.id + '_' + index)"
                        >
                          <Loader2 v-if="tool.status === 'running'" class="animate-spin w-3.5 h-3.5 text-emerald-400 shrink-0" />
                          <Check v-else-if="tool.status === 'complete'" class="w-3.5 h-3.5 text-emerald-400 shrink-0" />
                          <AlertCircle v-else class="w-3.5 h-3.5 text-red-400 shrink-0" />
                          <span class="font-mono text-xs truncate" :class="tool.status === 'error' ? 'text-red-300' : 'text-slate-300'">
                            <span class="text-slate-500">Call </span>{{ tool.name }}
                          </span>
                          <span v-if="tool.duration" class="text-[10px] text-slate-500 ml-1 shrink-0">{{ tool.duration }}s</span>
                          <ChevronDown v-if="expandedTools[msg.id + '_' + index]" class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                          <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                        </div>
                        
                        <!-- Tool details -->
                        <div v-if="expandedTools[msg.id + '_' + index]" class="p-2 pt-0 border-t border-slate-700/60 bg-slate-950/50">
                          <div class="text-[11px] font-semibold text-slate-500 mb-1 mt-2 uppercase tracking-wider">Arguments</div>
                          <pre class="bg-black/60 rounded p-1.5 font-mono text-[11px] text-slate-300 overflow-x-auto whitespace-pre-wrap break-all border-l-2 border-blue-500/50 custom-scrollbar m-0"><code>{{ tool.arguments || '{}' }}</code></pre>
  
                          <template v-if="tool.result">
                            <div class="text-[11px] font-semibold mb-1 mt-2 uppercase tracking-wider" :class="tool.status === 'error' ? 'text-red-400' : 'text-slate-500'">
                              {{ tool.status === 'error' ? 'Error' : 'Result' }}
                            </div>
                            <pre class="bg-black/60 rounded p-1.5 font-mono text-[11px] overflow-x-auto whitespace-pre-wrap break-all border-l-2 m-0"
                                 :class="tool.status === 'error' ? 'text-red-300 border-red-500/50' : 'text-slate-300 border-amber-500/50'"><code>{{ tool.result }}</code></pre>
                          </template>
                        </div>
                      </div>
                    </div>
                  </div>
                </template>

                <!-- Final response content (Legacy or fallback) -->
                <div
                  v-if="msg.content && (!msg.steps || !msg.steps.some((s: any) => s.type === 'content'))"
                  class="prose prose-invert max-w-none text-sm leading-relaxed transition-all relative group/content"
                  @click="copyCodeBlock"
                >
                  <!-- Copy entire message button -->
                  <button
                    @click.stop="copyMessage(msg)"
                    class="absolute -top-1 right-0 p-1.5 rounded-md bg-slate-800/80 border border-white/10 text-slate-400 hover:text-slate-200 hover:bg-slate-700/80 opacity-0 group-hover/msg:opacity-100 transition-all z-10"
                    title="Copy message"
                  >
                    <CheckCheck v-if="copiedMessageId === msg.id" class="w-3.5 h-3.5 text-emerald-400" />
                    <Copy v-else class="w-3.5 h-3.5" />
                  </button>
                  <div v-html="renderMarkdown(msg.content)"></div>
                </div>

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

      <!-- Scroll to bottom button -->
      <Transition
        enter-active-class="transition-all duration-200 ease-out"
        leave-active-class="transition-all duration-150 ease-in"
        enter-from-class="opacity-0 translate-y-2"
        leave-to-class="opacity-0 translate-y-2"
      >
        <button
          v-if="showScrollButton"
          @click="forceScrollToBottom"
          class="absolute bottom-36 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1.5 px-3 py-1.5 bg-slate-700/90 hover:bg-slate-600/90 border border-white/10 rounded-full text-slate-300 text-xs shadow-lg backdrop-blur-sm transition-colors"
        >
          <ArrowDown class="w-3.5 h-3.5" />
          Scroll to bottom
        </button>
      </Transition>

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
              @keydown="handleKeydown"
              @input="autoResize"
              :placeholder="sessionStatus === 'receiving' ? 'AI is processing your request...' : sessionStatus === 'sending' ? 'Sending your message...' : (localMessages.length > 0 ? `Ready for your next prompt... (${sendShortcut} to send)` : `Type your message... (${sendShortcut} to send)`)"
              :disabled="!isConnected || sessionStatus === 'receiving' || sessionStatus === 'sending'"
              autofocus
              rows="1"
              class="flex-1 overflow-x-hidden border-0 bg-transparent shadow-none focus:outline-none focus:ring-0 text-slate-100 px-3 py-2.5 disabled:opacity-50 disabled:cursor-not-allowed resize-none custom-scrollbar min-h-[44px] max-h-[400px]"
            ></textarea>

            <!-- Stop / Send button -->
            <Button
              v-if="sessionStatus === 'receiving' || isThinking"
              @click="stopGenerating"
              class="w-11 h-11 rounded-xl bg-red-500/80 hover:bg-red-500 text-white shrink-0 ml-2 transition-all"
              size="icon"
              title="Stop generating"
            >
              <Square class="w-4 h-4 fill-current" />
            </Button>
            <Button
              v-else
              @click="sendMessage"
              :disabled="!inputValue.trim() || !isConnected || sessionStatus === 'sending'"
              class="w-11 h-11 rounded-xl text-white shrink-0 ml-2 transition-all"
              :class="{
                'bg-blue-500 hover:bg-blue-400': sessionStatus === 'idle',
                'bg-slate-600 cursor-not-allowed': sessionStatus !== 'idle'
              }"
              size="icon"
            >
              <Send class="w-5 h-5" />
            </Button>
          </div>
          <div class="flex items-center justify-between text-xs text-slate-500 mt-3 px-1">
            <span class="font-medium">Powered by Nanobot-rs Web Gateway</span>
            <button
              @click="sendOnEnter = !sendOnEnter"
              class="hover:text-slate-300 transition-colors"
              :title="sendOnEnter ? 'Click to switch to Cmd+Enter to send' : 'Click to switch to Enter to send'"
            >
              {{ sendOnEnter ? `${isMac ? 'Shift' : 'Shift'}+Enter for new line` : `Enter for new line` }}
            </button>
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
.prose code { background-color: rgba(0,0,0,0.3); padding: 0.2em 0.4em; border-radius: 4px; font-family: 'Menlo', 'Monaco', 'Courier New', monospace; font-size: 0.9em; color: #e2e8f0; }

/* Code block with copy button wrapper */
.prose pre {
  background-color: rgba(0,0,0,0.4);
  padding: 12px;
  border-radius: 8px;
  overflow-x: auto;
  margin: 0.75em 0;
  border: 1px solid rgba(255,255,255,0.1);
  position: relative;
}
.prose pre code { background-color: transparent; padding: 0; font-size: 0.9em; }

/* Highlight.js theme overrides for dark mode */
.hljs { background: transparent !important; color: #e2e8f0; }
.hljs-keyword, .hljs-selector-tag { color: #c792ea; }
.hljs-string, .hljs-attr { color: #c3e88d; }
.hljs-number, .hljs-literal { color: #f78c6c; }
.hljs-comment { color: #546e7a; font-style: italic; }
.hljs-function .hljs-title, .hljs-title.function_ { color: #82aaff; }
.hljs-built_in { color: #ffcb6b; }
.hljs-type, .hljs-class .hljs-title { color: #ffcb6b; }
.hljs-params { color: #89ddff; }
.hljs-meta { color: #f78c6c; }
.hljs-tag { color: #f07178; }
.hljs-name { color: #f07178; }
.hljs-attribute { color: #c792ea; }
.hljs-symbol { color: #82aaff; }
.hljs-variable { color: #f07178; }
.hljs-deletion { color: #f07178; background-color: rgba(244, 67, 54, 0.1); }
.hljs-addition { color: #c3e88d; background-color: rgba(76, 175, 80, 0.1); }

/* Custom Scrollbar */
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

/* Mermaid diagram styling */
.mermaid-container {
  background: rgba(15, 23, 42, 0.6);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 8px;
  padding: 16px;
  margin: 0.75em 0;
  overflow-x: auto;
  display: flex;
  justify-content: center;
}
.mermaid-container pre.mermaid {
  background: transparent !important;
  border: none !important;
  padding: 0 !important;
  margin: 0 !important;
  overflow: visible;
}
.mermaid-container svg {
  max-width: 100%;
  height: auto;
}
</style>
