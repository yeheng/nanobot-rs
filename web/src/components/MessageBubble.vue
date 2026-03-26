<script setup lang="ts">
import { ref, watch, nextTick } from 'vue';
import { ChevronDown, ChevronRight, Check, Brain, Copy, CheckCheck, Loader2, AlertCircle } from 'lucide-vue-next';
import { Marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import hljs from 'highlight.js';
import DOMPurify from 'dompurify';
import mermaid from 'mermaid';
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

// Configure marked with highlight.js + mermaid code block handling
const mermaidRenderer = {
  code({ text, lang }: { text: string; lang?: string }) {
    if (lang === 'mermaid') {
      const id = `mermaid-${Date.now()}-${mermaidIdCounter++}`;
      return `<div class="mermaid-container"><pre class="mermaid" id="${id}">${text}</pre></div>`;
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

const props = defineProps<{
  message: Message;
  isLastBotMessage: boolean;
  isThinking: boolean;
}>();

const emit = defineEmits<{
  (e: 'copy-message', message: Message): void;
}>();

// UI State
const expandedTools = ref<Record<string, boolean>>({});
const expandedThinking = ref<Record<string, boolean>>({});
const copiedMessageId = ref<string | null>(null);

// Get tool progress
const getToolProgress = (tools: any[]) => {
  const completed = tools.filter(t => t.status === 'complete' || t.status === 'error').length;
  return { completed, total: tools.length };
};

// Toggle functions
const toggleThinkingExpand = (id: string) => {
  if (expandedThinking.value[id] === undefined) {
    expandedThinking.value[id] = false;
  } else {
    expandedThinking.value[id] = !expandedThinking.value[id];
  }
};

const toggleToolExpand = (key: string) => {
  expandedTools.value[key] = !expandedTools.value[key];
};

// Render markdown
const renderMarkdown = (text: string) => {
  if (!text) return '';
  const rawHtml = markedInstance.parse(text) as string;
  return DOMPurify.sanitize(rawHtml, {
    ADD_TAGS: ['pre'],
    ADD_ATTR: ['class', 'id'],
  });
};

// Render mermaid diagrams
const renderMermaidDiagrams = async () => {
  await nextTick();
  try {
    const elements = document.querySelectorAll('pre.mermaid:not([data-processed])');
    if (elements.length > 0) {
      await mermaid.run({ nodes: elements as any });
    }
  } catch (e) {
    console.warn('Mermaid rendering error:', e);
  }
};

// Copy message
const copyMessage = async () => {
  try {
    await navigator.clipboard.writeText(props.message.content);
    copiedMessageId.value = props.message.id;
    emit('copy-message', props.message);
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
  } catch (e) {
    console.error('Failed to copy code:', e);
  }
};

// Watch for changes to render mermaid
watch(() => props.message, () => {
  renderMermaidDiagrams();
}, { deep: true });

// Format time
const formatTime = (timestamp: number) => {
  return new Date(timestamp).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
};
</script>

<template>
  <div
    :id="`msg-${message.id}`"
    class="flex flex-col animate-in fade-in slide-in-from-bottom-2 duration-300 w-full group/msg"
    :class="message.role === 'user' ? 'self-end max-w-[85%]' : (message.role === 'system' ? 'self-center max-w-[95%]' : 'self-start w-full')"
  >
    <div v-if="message.role !== 'system'" class="flex items-center gap-2 mb-1.5" :class="message.role === 'user' ? 'justify-end mx-1' : 'ml-0'">
      <span class="text-xs" :class="message.role === 'user' ? 'text-slate-400' : 'font-semibold text-slate-300'">
        {{ message.role === 'user' ? 'You' : 'Nanobot' }}
      </span>
      <span class="text-[10px] text-slate-500" :title="new Date(message.timestamp).toLocaleString()">
        {{ formatTime(message.timestamp) }}
      </span>
    </div>

    <div class="relative break-words transition-all" :class="{
      'rounded-2xl bg-gradient-to-br from-blue-500 to-blue-700 text-white p-3 px-4 rounded-br-sm shadow-lg shadow-blue-500/20': message.role === 'user',
      'w-full py-1 text-slate-200': message.role === 'bot',
      'rounded-xl bg-black/20 text-slate-400 py-1.5 px-3 text-xs text-center mx-auto': message.role === 'system'
    }">
      <!-- System message -->
      <div v-if="message.role === 'system'" class="text-xs">
        {{ message.content }}
      </div>

      <!-- Bot message structure -->
      <template v-else-if="message.role === 'bot'">
        <!-- Steps section (interleaved thinking and tools) -->
        <template v-if="message.steps && message.steps.length > 0">
          <div v-for="(step, stepIdx) in message.steps" :key="step.id" class="mb-4">
            <!-- Thinking Step -->
            <div v-if="step.type === 'thinking'" class="pb-4" :class="{ 'border-b border-white/10': stepIdx < message.steps.length - 1 || message.content }">
              <div
                class="flex items-center gap-2 mb-2 cursor-pointer select-none hover:bg-slate-700/30 p-1.5 -ml-1.5 rounded transition-colors"
                @click="toggleThinkingExpand(step.id)"
              >
                <Brain class="w-4 h-4 text-violet-400" :class="{ 'animate-pulse': isThinking && isLastBotMessage && stepIdx === message.steps.length - 1 }" />
                <span class="text-xs font-medium text-violet-400">
                  {{ isThinking && isLastBotMessage && stepIdx === message.steps.length - 1 ? 'Thinking...' : 'Thinking Process' }}
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
                    'w-full': expandedTools[message.id + '_' + stepIdx + '_' + index],
                    'w-auto max-w-full': !expandedTools[message.id + '_' + stepIdx + '_' + index]
                  }"
                >
                  <!-- Tool header -->
                  <div
                    class="flex items-center gap-2 p-1.5 px-2.5 cursor-pointer hover:bg-slate-700/40 transition-colors select-none"
                    @click="toggleToolExpand(message.id + '_' + stepIdx + '_' + index)"
                  >
                    <Loader2 v-if="tool.status === 'running'" class="animate-spin w-3.5 h-3.5 text-emerald-400 shrink-0" />
                    <Check v-else-if="tool.status === 'complete'" class="w-3.5 h-3.5 text-emerald-400 shrink-0" />
                    <AlertCircle v-else class="w-3.5 h-3.5 text-red-400 shrink-0" />
                    <span class="font-mono text-xs truncate" :class="tool.status === 'error' ? 'text-red-300' : 'text-slate-300'">
                      <span class="text-slate-500">Call </span>{{ tool.name }}
                    </span>
                    <span v-if="tool.duration" class="text-[10px] text-slate-500 ml-1 shrink-0">{{ tool.duration }}s</span>
                    <ChevronDown v-if="expandedTools[message.id + '_' + stepIdx + '_' + index]" class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                    <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0 ml-1" />
                  </div>

                  <!-- Tool details -->
                  <div v-if="expandedTools[message.id + '_' + stepIdx + '_' + index]" class="p-2 pt-0 border-t border-slate-700/60 bg-slate-950/50">
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
                  v-if="stepIdx === message.steps.length - 1 || !message.steps.slice(stepIdx + 1).some((s: any) => s.type === 'content')"
                  @click.stop="copyMessage"
                  class="absolute -top-1 right-0 p-1.5 rounded-md bg-slate-800/80 border border-white/10 text-slate-400 hover:text-slate-200 hover:bg-slate-700/80 opacity-0 group-hover/msg:opacity-100 transition-all z-10"
                  title="Copy message"
                >
                  <CheckCheck v-if="copiedMessageId === message.id" class="w-3.5 h-3.5 text-emerald-400" />
                  <Copy v-else class="w-3.5 h-3.5" />
                </button>
                <div v-html="renderMarkdown(step.content)"></div>
              </div>
            </div>
          </div>
        </template>
      </template>

      <!-- User message -->
      <div v-else class="text-sm">
        {{ message.content }}
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Markdown specific styling */
.prose p { margin-bottom: 0.75em; }
.prose p:last-child { margin-bottom: 0; }
.prose a { color: #60a5fa; text-decoration: none; }
.prose a:hover { text-decoration: underline; }
.prose code { background-color: rgba(0,0,0,0.3); padding: 0.2em 0.4em; border-radius: 4px; font-family: 'Menlo', 'Monaco', 'Courier New', monospace; font-size: 0.9em; color: #e2e8f0; }

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

.hljs { background: transparent !important; color: #e2e8f0; }
.hljs-keyword, .hljs-selector-tag { color: #c792ea; }
.hljs-string, .hljs-attr { color: #c3e88d; }
.hljs-number, .hljs-literal { color: #f78c6c; }
.hljs-comment { color: #546e7a; font-style: italic; }
.hljs-function .hljs-title, .hljs-title.function_ { color: #82aaff; }
.hljs-built_in { color: #ffcb6b; }

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
