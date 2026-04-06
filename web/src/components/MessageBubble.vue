<script setup lang="ts">
import DOMPurify from 'dompurify';
import hljs from 'highlight.js';
import { AlertCircle, Brain, Check, CheckCheck, ChevronDown, ChevronRight, Copy, Loader2, MessageSquare, Wrench } from 'lucide-vue-next';
import { Marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import mermaid from 'mermaid';
import { computed, nextTick, ref, watch } from 'vue';
import type { Message } from '../App.vue';

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

// Configure marked
const mermaidRenderer = {
  code({ text, lang }: { text: string; lang?: string }) {
    if (lang === 'mermaid') {
      // Use more reliable ID generation with random suffix to avoid conflicts
      const id = `mermaid-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
      return `<div class="mermaid-container"><pre class="mermaid" id="${id}" data-processed="false">${text}</pre></div>`;
    }
    return false;
  }
};

const markedInstance = new Marked(
  markedHighlight({
    langPrefix: 'hljs language-',
    highlight(code: string, lang: string) {
      if (lang === 'mermaid') return code;
      if (lang && hljs.getLanguage(lang)) {
        try { return hljs.highlight(code, { language: lang }).value; } catch (__) {}
      }
      try { return hljs.highlightAuto(code).value; } catch (__) {}
      return code;
    }
  })
);
markedInstance.use({ renderer: mermaidRenderer });
markedInstance.setOptions({ breaks: true, gfm: true });

const props = defineProps<{
  message: Message;
  isLastBotMessage: boolean;
  isThinking: boolean;
  isReceiving: boolean;
}>();

const emit = defineEmits<{
  (e: 'copy-message', message: Message): void;
}>();

// UI State - independent expand states for each section
const thinkingExpanded = ref(true);
const toolsExpanded = ref(false);

// Tool expand states
const expandedTools = ref<Record<string, boolean>>({});
const copiedMessageId = ref<string | null>(null);

// Computed: Tool progress
const toolProgress = computed(() => {
  if (!props.message.toolCalls?.length) return { completed: 0, total: 0, hasError: false };
  const tools = props.message.toolCalls;
  const completed = tools.filter(t => t.status === 'complete' || t.status === 'error').length;
  const hasError = tools.some(t => t.status === 'error');
  return { completed, total: tools.length, hasError };
});

// Computed: Is thinking active (streaming)
const isThinkingActive = computed(() => props.isThinking && props.isLastBotMessage);

// Computed: Is tool calls active
const isToolsActive = computed(() => {
  if (!props.message.toolCalls?.length) return false;
  return props.message.toolCalls.some(t => t.status === 'running');
});

// Toggle functions
const toggleThinking = () => { thinkingExpanded.value = !thinkingExpanded.value; };
const toggleTools = () => { toolsExpanded.value = !toolsExpanded.value; };
const toggleToolExpand = (key: string) => { expandedTools.value[key] = !expandedTools.value[key]; };

// Render markdown
const renderMarkdown = (text: string) => {
  if (!text) return '';
  const rawHtml = markedInstance.parse(text) as string;
  return DOMPurify.sanitize(rawHtml, { ADD_TAGS: ['pre'], ADD_ATTR: ['class', 'id'] });
};

// Render mermaid diagrams with fallback handling
const renderMermaidDiagrams = async () => {
  await nextTick();
  try {
    const elements = document.querySelectorAll('pre.mermaid:not([data-processed])');
    if (elements.length > 0) {
      await mermaid.run({ 
        nodes: elements as any,
        suppressErrors: true 
      });
    }
  } catch (e) {
    console.warn('Mermaid rendering error:', e);
    // Fallback: add error class to failed diagrams for visual feedback
    const failedElements = document.querySelectorAll('pre.mermaid:not([data-processed])');
    failedElements.forEach(el => {
      el.classList.add('mermaid-error');
      el.title = 'Failed to render diagram. Click to see raw code.';
      el.style.cursor = 'pointer';
      el.addEventListener('click', () => {
        el.classList.toggle('show-raw');
      });
    });
  }
};

// Copy message
const copyMessage = async () => {
  try {
    await navigator.clipboard.writeText(props.message.content);
    copiedMessageId.value = props.message.id;
    emit('copy-message', props.message);
    setTimeout(() => { copiedMessageId.value = null; }, 2000);
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
  try {
    await navigator.clipboard.writeText(pre.textContent || '');
  } catch (e) {
    console.error('Failed to copy code:', e);
  }
};

// Watch for changes
watch(() => props.message, () => { renderMermaidDiagrams(); }, { deep: true });

// Format time
const formatTime = (timestamp: number) => {
  return new Date(timestamp).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
};

// Has thinking content
const hasThinking = computed(() => props.message.thinking && props.message.thinking.trim().length > 0);

// Has tool calls
const hasToolCalls = computed(() => props.message.toolCalls && props.message.toolCalls.length > 0);

// Has content
const hasContent = computed(() => props.message.content && props.message.content.trim().length > 0);
</script>

<template>
  <div
    :id="`msg-${message.id}`"
    class="flex flex-col animate-in fade-in slide-in-from-bottom-2 duration-300 w-full group/msg"
    :class="message.role === 'user' ? 'self-end max-w-[85%]' : (message.role === 'system' ? 'self-center max-w-[95%]' : 'self-start w-full')"
  >
    <!-- Header -->
    <div v-if="message.role !== 'system'" class="flex items-center gap-2 mb-2" :class="message.role === 'user' ? 'justify-end mx-1' : 'ml-0'">
      <span class="text-xs" :class="message.role === 'user' ? 'text-slate-400' : 'font-semibold text-slate-300'">
        {{ message.role === 'user' ? 'You' : 'gasket' }}
      </span>
      <span class="text-[10px] text-slate-500" :title="new Date(message.timestamp).toLocaleString()">
        {{ formatTime(message.timestamp) }}
      </span>
    </div>

    <!-- User / System message -->
    <div v-if="message.role !== 'bot'" class="relative break-words transition-all" :class="{
      'rounded-2xl bg-gradient-to-br from-blue-500 to-blue-700 text-white p-3 px-4 rounded-br-sm shadow-lg shadow-blue-500/20': message.role === 'user',
      'rounded-xl bg-black/20 text-slate-400 py-1.5 px-3 text-xs text-center mx-auto': message.role === 'system'
    }">
      <div v-if="message.role === 'system'" class="text-xs">{{ message.content }}</div>
      <div v-else class="text-sm whitespace-pre-wrap">{{ message.content }}</div>
    </div>

    <!-- Bot message: Separated sections -->
    <template v-else>
      <!-- Section 1: Thinking Process (Isolated Box) -->
      <div v-if="hasThinking || isThinkingActive" class="mb-3">
        <div
          class="bg-violet-950/30 border border-violet-500/30 rounded-xl overflow-hidden transition-all"
          :class="{ 'ring-2 ring-violet-500/50': isThinkingActive }"
        >
          <!-- Header -->
          <div
            class="flex items-center gap-2 px-3 py-2 bg-violet-900/30 cursor-pointer hover:bg-violet-900/50 transition-colors select-none"
            @click="toggleThinking"
          >
            <Brain class="w-4 h-4 text-violet-400 shrink-0" :class="{ 'animate-pulse': isThinkingActive }" />
            <span class="text-xs font-medium text-violet-300 flex-1">
              {{ isThinkingActive ? 'Thinking...' : 'Thinking Process' }}
            </span>
            <ChevronDown v-if="thinkingExpanded" class="w-4 h-4 text-violet-400 shrink-0" />
            <ChevronRight v-else class="w-4 h-4 text-violet-400 shrink-0" />
          </div>
          <!-- Content -->
          <div v-show="thinkingExpanded" class="px-3 py-2 max-h-60 overflow-y-auto custom-scrollbar">
            <div v-if="isThinkingActive && !hasThinking" class="flex items-center gap-2 text-violet-300/60 text-sm">
              <Loader2 class="w-4 h-4 animate-spin" />
              <span>Processing your request...</span>
            </div>
            <div v-else class="text-[13px] text-violet-200/80 italic whitespace-pre-wrap leading-relaxed">
              {{ message.thinking }}
            </div>
          </div>
        </div>
      </div>

      <!-- Section 2: Tool Calls (Isolated Box) -->
      <div v-if="hasToolCalls" class="mb-3">
        <div
          class="bg-slate-800/50 border border-slate-700/50 rounded-xl overflow-hidden transition-all"
          :class="{ 'ring-2 ring-emerald-500/30': isToolsActive }"
        >
          <!-- Header -->
          <div
            class="flex items-center gap-2 px-3 py-2 bg-slate-800/80 cursor-pointer hover:bg-slate-700/80 transition-colors select-none"
            @click="toggleTools"
          >
            <Wrench class="w-4 h-4 text-emerald-400 shrink-0" />
            <span class="text-xs font-medium text-slate-300 flex-1">
              Tool Calls
            </span>
            <!-- Progress badge -->
            <span
              class="text-[10px] px-2 py-0.5 rounded-full font-mono"
              :class="toolProgress.hasError ? 'bg-red-500/20 text-red-400' : (toolProgress.completed === toolProgress.total ? 'bg-emerald-500/20 text-emerald-400' : 'bg-amber-500/20 text-amber-400')"
            >
              {{ toolProgress.completed }}/{{ toolProgress.total }}
            </span>
            <ChevronDown v-if="toolsExpanded" class="w-4 h-4 text-slate-400 shrink-0" />
            <ChevronRight v-else class="w-4 h-4 text-slate-400 shrink-0" />
          </div>
          <!-- Content: Tool list -->
          <div v-show="toolsExpanded" class="px-3 py-2 space-y-2">
            <div
              v-for="(tool, idx) in message.toolCalls"
              :key="tool.id || idx"
              class="border rounded-lg overflow-hidden"
              :class="tool.status === 'error' ? 'border-red-500/30 bg-red-950/20' : 'border-slate-700/50 bg-slate-900/50'"
            >
              <!-- Tool header -->
              <div
                class="flex items-center gap-2 px-2 py-1.5 cursor-pointer hover:bg-slate-700/30 transition-colors select-none"
                @click="toggleToolExpand(tool.id || `tool_${idx}`)"
              >
                <Loader2 v-if="tool.status === 'running'" class="animate-spin w-3.5 h-3.5 text-emerald-400 shrink-0" />
                <Check v-else-if="tool.status === 'complete'" class="w-3.5 h-3.5 text-emerald-400 shrink-0" />
                <AlertCircle v-else class="w-3.5 h-3.5 text-red-400 shrink-0" />
                <span class="font-mono text-xs flex-1" :class="tool.status === 'error' ? 'text-red-300' : 'text-slate-300'">
                  {{ tool.name }}
                </span>
                <span v-if="tool.duration" class="text-[10px] text-slate-500">{{ tool.duration }}s</span>
                <ChevronDown v-if="expandedTools[tool.id || `tool_${idx}`]" class="w-3.5 h-3.5 text-slate-500 shrink-0" />
                <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0" />
              </div>
              <!-- Tool details -->
              <div v-if="expandedTools[tool.id || `tool_${idx}`]" class="px-2 pb-2 pt-0 border-t border-slate-700/30">
                <div class="text-[10px] font-semibold text-slate-500 mt-2 uppercase tracking-wider">Arguments</div>
                <pre class="bg-black/40 rounded p-2 font-mono text-[11px] text-slate-300 overflow-x-auto whitespace-pre-wrap break-all mt-1 border-l-2 border-blue-500/40"><code>{{ tool.arguments || '{}' }}</code></pre>
                <template v-if="tool.result">
                  <div class="text-[10px] font-semibold mt-2 uppercase tracking-wider" :class="tool.status === 'error' ? 'text-red-400' : 'text-slate-500'">
                    {{ tool.status === 'error' ? 'Error' : 'Result' }}
                  </div>
                  <pre class="bg-black/40 rounded p-2 font-mono text-[11px] overflow-x-auto whitespace-pre-wrap break-all mt-1 border-l-2" :class="tool.status === 'error' ? 'text-red-300 border-red-500/40' : 'text-slate-300 border-amber-500/40'"><code>{{ tool.result }}</code></pre>
                </template>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Section 3: Final Content (Main Response) -->
      <div v-if="hasContent || isReceiving" class="relative">
        <!-- Content header -->
        <div class="flex items-center gap-2 mb-2">
          <MessageSquare class="w-4 h-4 text-blue-400" />
          <span class="text-xs font-medium text-slate-300">Response</span>
          <!-- Copy button -->
          <button
            v-if="hasContent"
            @click.stop="copyMessage"
            class="ml-auto p-1.5 rounded-md bg-slate-800/80 border border-white/10 text-slate-400 hover:text-slate-200 hover:bg-slate-700/80 opacity-0 group-hover/msg:opacity-100 transition-all"
            title="Copy message"
          >
            <CheckCheck v-if="copiedMessageId === message.id" class="w-3.5 h-3.5 text-emerald-400" />
            <Copy v-else class="w-3.5 h-3.5" />
          </button>
        </div>
        <!-- Content body -->
        <div
          class="prose prose-invert max-w-none text-sm leading-relaxed transition-all bg-slate-800/30 rounded-xl p-4 border border-slate-700/30"
          @click="copyCodeBlock"
        >
          <div v-if="!hasContent && isReceiving" class="flex items-center gap-2 text-slate-400">
            <div class="flex gap-1">
              <span class="w-1.5 h-1.5 bg-blue-500 rounded-full animate-bounce [animation-delay:0s]"></span>
              <span class="w-1.5 h-1.5 bg-blue-500 rounded-full animate-bounce [animation-delay:-0.15s]"></span>
              <span class="w-1.5 h-1.5 bg-blue-500 rounded-full animate-bounce [animation-delay:-0.3s]"></span>
            </div>
            <span class="text-xs">Generating response...</span>
          </div>
          <div v-else v-html="renderMarkdown(message.content)"></div>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
/* Markdown styling */
.prose p { margin-bottom: 0.75em; }
.prose p:last-child { margin-bottom: 0; }
.prose a { color: #60a5fa; text-decoration: none; }
.prose a:hover { text-decoration: underline; }
.prose code { background-color: rgba(0,0,0,0.3); padding: 0.2em 0.4em; border-radius: 4px; font-family: 'Menlo', 'Monaco', 'Courier New', monospace; font-size: 0.9em; color: #e2e8f0; }
.prose pre { background-color: rgba(0,0,0,0.4); padding: 12px; border-radius: 8px; overflow-x: auto; margin: 0.75em 0; border: 1px solid rgba(255,255,255,0.1); position: relative; }
.prose pre code { background-color: transparent; padding: 0; font-size: 0.9em; }

/* Highlight.js */
.hljs { background: transparent !important; color: #e2e8f0; }
.hljs-keyword, .hljs-selector-tag { color: #c792ea; }
.hljs-string, .hljs-attr { color: #c3e88d; }
.hljs-number, .hljs-literal { color: #f78c6c; }
.hljs-comment { color: #546e7a; font-style: italic; }
.hljs-function .hljs-title, .hljs-title.function_ { color: #82aaff; }
.hljs-built-in { color: #ffcb6b; }

/* Mermaid */
.mermaid-container { background: rgba(15, 23, 42, 0.6); border: 1px solid rgba(255, 255, 255, 0.08); border-radius: 8px; padding: 16px; margin: 0.75em 0; overflow-x: auto; display: flex; justify-content: center; }
.mermaid-container pre.mermaid { background: transparent !important; border: none !important; padding: 0 !important; margin: 0 !important; overflow: visible; }
.mermaid-container svg { max-width: 100%; height: auto; }

/* Scrollbar */
.custom-scrollbar::-webkit-scrollbar { width: 6px; }
.custom-scrollbar::-webkit-scrollbar-track { background: rgba(0,0,0,0.1); border-radius: 4px; }
.custom-scrollbar::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.2); border-radius: 4px; }
.custom-scrollbar::-webkit-scrollbar-thumb:hover { background: rgba(255,255,255,0.3); }
</style>
