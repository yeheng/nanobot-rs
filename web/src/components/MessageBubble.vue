<script setup lang="ts">
import DOMPurify from 'dompurify';
import hljs from 'highlight.js';
import 'highlight.js/styles/github-dark.css';
import { AlertCircle, Bot, Check, CheckCheck, User } from 'lucide-vue-next';
import { marked } from 'marked';
import { markedHighlight } from 'marked-highlight';
import mermaid from 'mermaid';
import { computed, nextTick, ref, watch } from 'vue';
import type { Message } from '../types';
import MessageThoughtsPanel from './MessageThoughtsPanel.vue';

const props = defineProps<{
  message: Message;
  isLastBotMessage: boolean;
  isThinking: boolean;
  isReceiving: boolean;
}>();

const emit = defineEmits<{
  (e: 'retry'): void;
}>();

// Setup marked
marked.use(
  markedHighlight({
    emptyLangClass: 'hljs',
    langPrefix: 'hljs language-',
    highlight(code, lang) {
      const language = hljs.getLanguage(lang) ? lang : 'plaintext';
      return hljs.highlight(code, { language }).value;
    },
  })
);

marked.setOptions({
  breaks: true,
  gfm: true,
});

// Mermaid rendering
const mermaidContainerRef = ref<HTMLDivElement | null>(null);

const renderMermaid = async () => {
  await nextTick();
  if (!mermaidContainerRef.value) return;
  const elements = mermaidContainerRef.value.querySelectorAll('.mermaid-diagram');
  for (const el of elements) {
    try {
      const { svg } = await mermaid.render(
        `mermaid-${Math.random().toString(36).substr(2, 9)}`,
        decodeURIComponent((el as HTMLElement).dataset.source || '')
      );
      (el as HTMLElement).innerHTML = svg;
    } catch (e) {
      console.error('Mermaid render error:', e);
    }
  }
};

const customRenderer = new marked.Renderer();
customRenderer.code = (codeObj: any) => {
  const code = typeof codeObj === 'string' ? codeObj : codeObj.text || '';
  const lang = typeof codeObj === 'string' ? '' : codeObj.lang || '';
  const trimmed = code.trim();
  if (lang === 'mermaid' || trimmed.startsWith('graph ') || trimmed.startsWith('sequenceDiagram') || trimmed.startsWith('classDiagram')) {
    return `<div class="mermaid-diagram my-2 flex justify-center" data-source="${encodeURIComponent(trimmed)}"></div>`;
  }
  const highlighted = hljs.highlightAuto(code).value;
  return `<div class="relative group my-2"><button class="copy-btn absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity bg-black/20 hover:bg-black/30 dark:bg-white/10 dark:hover:bg-white/20 text-gray-800 dark:text-white/80 text-[10px] px-2 py-1 rounded backdrop-blur-sm">Copy</button><pre class="hljs rounded-lg p-3 overflow-x-auto text-xs"><code class="language-${lang}">${highlighted}</code></pre></div>`;
};

const unescapeRecursive = (str: string): string => {
  const decoded = str
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&#x27;/g, "'");
  return decoded === str ? str : unescapeRecursive(decoded);
};

const parsedContent = computed(() => {
  if (!props.message.content) return '';
  const decoded = unescapeRecursive(props.message.content);
  const raw = marked.parse(decoded, { renderer: customRenderer }) as string;
  return DOMPurify.sanitize(raw);
});

watch(() => props.message.content, async () => {
  await renderMermaid();
}, { immediate: true });

const copyCode = (event: MouseEvent) => {
  const btn = event.target as HTMLElement;
  const pre = btn.closest('.relative')?.querySelector('pre');
  if (pre) {
    const code = pre.textContent || '';
    navigator.clipboard.writeText(code).then(() => {
      btn.textContent = 'Copied!';
      setTimeout(() => (btn.textContent = 'Copy'), 1500);
    });
  }
};

const formatTime = (ts: number) => {
  return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
};

const isStreaming = computed(() => props.isLastBotMessage && props.isReceiving);
</script>

<template>
  <div class="py-1"
    :class="message.role === 'user' ? 'flex justify-end' : message.role === 'system' ? 'flex justify-center' : 'flex justify-start'">
    
    <!-- System message -->
    <div v-if="message.role === 'system'" class="text-[10px] text-gray-500 dark:text-slate-500 px-3 py-1 bg-gray-100 dark:bg-slate-800/40 rounded-full">
      {{ message.content }}
    </div>

    <!-- User message -->
    <div v-else-if="message.role === 'user'" class="flex items-end gap-2 max-w-[85%] md:max-w-[75%]">
      <div class="flex flex-col items-end gap-0.5">
        <div class="px-4 py-2.5 rounded-2xl rounded-br-sm bg-gradient-to-br from-blue-600 to-blue-500 text-white text-sm shadow-sm">
          <div class="whitespace-pre-wrap">{{ message.content }}</div>
        </div>
        <div class="flex items-center gap-1 px-1">
          <span class="text-[10px] text-gray-400 dark:text-slate-500">{{ formatTime(message.timestamp) }}</span>
          <Check v-if="message.status === 'sending'" class="w-3 h-3 text-gray-400 dark:text-slate-500" />
          <CheckCheck v-else-if="message.status === 'sent'" class="w-3 h-3 text-gray-400 dark:text-slate-500" />
          <div v-else-if="message.status === 'error'" class="flex items-center gap-1 text-red-500 dark:text-red-400 cursor-pointer hover:underline" @click="emit('retry')">
            <AlertCircle class="w-3 h-3" />
            <span class="text-[10px]">Failed</span>
          </div>
        </div>
      </div>
      <div class="w-7 h-7 rounded-full bg-gray-300 dark:bg-slate-600 flex items-center justify-center shrink-0">
        <User class="w-3.5 h-3.5 text-gray-700 dark:text-slate-200" />
      </div>
    </div>

    <!-- Bot message -->
    <div v-else class="flex items-start gap-2 w-full">
      <div class="w-7 h-7 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center shrink-0 mt-0.5">
        <Bot class="w-3.5 h-3.5 text-white" />
      </div>
      <div class="flex flex-col gap-1 min-w-0 flex-1 pr-4">
        <div class="flex items-center gap-2">
          <span class="text-xs font-medium text-gray-800 dark:text-slate-300">AI</span>
          <span class="text-[10px] text-gray-400 dark:text-slate-500">{{ formatTime(message.timestamp) }}</span>
        </div>

        <MessageThoughtsPanel
          :message="message"
          :is-thinking="isThinking"
          :is-last-bot-message="isLastBotMessage"
        />

        <!-- Content -->
        <div v-if="message.content || isStreaming"
          class="px-4 py-2.5 rounded-2xl rounded-tl-sm bg-white dark:bg-slate-700/50 text-gray-900 dark:text-slate-100 text-sm border border-gray-200 dark:border-white/5 shadow-sm min-w-0 max-w-[95%] md:max-w-[85%]">
          <div class="prose prose-invert prose-sm max-w-none" v-html="parsedContent" @click="copyCode" />
          <!-- Streaming cursor -->
          <span v-if="isStreaming" class="inline-block w-2 h-4 bg-blue-500/80 dark:bg-blue-400/80 ml-0.5 align-middle animate-pulse rounded-sm" />
        </div>
      </div>
    </div>
  </div>
</template>

<style>
.prose pre { margin: 0; }
.prose p { margin-bottom: 0.5em; }
.prose p:last-child { margin-bottom: 0; }
</style>
