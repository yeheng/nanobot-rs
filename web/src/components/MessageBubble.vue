<script setup lang="ts">
import DOMPurify from 'dompurify';
import hljs from 'highlight.js';
import 'highlight.js/styles/github-dark.css';
import { AlertCircle, Bot, Check, CheckCheck, User } from 'lucide-vue-next';
import { useTheme } from '../composables/useTheme';
import { marked } from 'marked';
// marked-highlight removed — customRenderer.code handles highlighting directly
import { computed, nextTick, ref, watch } from 'vue';
import type { Message } from '../types';
import MessageThoughtsPanel from './MessageThoughtsPanel.vue';

// Module-level marked setup — runs once, shared by all instances
const customRenderer = new marked.Renderer();
customRenderer.code = (codeObj: any) => {
  const code = typeof codeObj === 'string' ? codeObj : codeObj.text || '';
  const lang = typeof codeObj === 'string' ? '' : codeObj.lang || '';
  const trimmed = code.trim();
  if (lang === 'mermaid' || trimmed.startsWith('graph ') || trimmed.startsWith('sequenceDiagram') || trimmed.startsWith('classDiagram')) {
    return `<div class="mermaid-diagram my-2 flex justify-center" data-source="${encodeURIComponent(trimmed)}"></div>`;
  }
  const highlighted = hljs.highlightAuto(code).value;
  return `<div class="relative group my-2"><button class="copy-btn absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity bg-secondary/60 hover:bg-secondary/80 text-secondary-foreground text-[10px] px-2 py-1 rounded backdrop-blur-sm">Copy</button><pre class="hljs rounded-lg p-3 overflow-x-auto text-xs"><code class="language-${lang}">${highlighted}</code></pre></div>`;
};

marked.setOptions({
  breaks: true,
  gfm: true,
  renderer: customRenderer,
});

const props = defineProps<{
  message: Message;
  isLastBotMessage: boolean;
  isThinking: boolean;
  isReceiving: boolean;
}>();

const emit = defineEmits<{
  (e: 'retry'): void;
}>();

const { markdownStyle } = useTheme();

// Mermaid rendering — lazy loaded to avoid init-time crashes
const mermaidContainerRef = ref<HTMLDivElement | null>(null);
let mermaidModule: typeof import('mermaid') | null = null;

const renderMermaid = async () => {
  await nextTick();
  if (!mermaidContainerRef.value) return;
  const elements = mermaidContainerRef.value.querySelectorAll('.mermaid-diagram');
  if (elements.length === 0) return;

  // Lazy-load mermaid only when needed
  if (!mermaidModule) {
    try {
      mermaidModule = await import('mermaid');
      mermaidModule.default.initialize({ startOnLoad: false, securityLevel: 'strict' });
    } catch (e) {
      console.error('Failed to load mermaid:', e);
      return;
    }
  }

  for (const el of elements) {
    // Skip already rendered diagrams
    if ((el as HTMLElement).dataset.rendered === 'true') continue;
    try {
      const source = decodeURIComponent((el as HTMLElement).dataset.source || '');
      if (!source) continue;
      const { svg } = await mermaidModule.default.render(
        `mermaid-${Math.random().toString(36).substr(2, 9)}`,
        source
      );
      (el as HTMLElement).innerHTML = svg;
      (el as HTMLElement).dataset.rendered = 'true';
    } catch (e) {
      console.error('Mermaid render error:', e);
      (el as HTMLElement).dataset.rendered = 'true';
    }
  }
};

// Maximum characters to parse as markdown; beyond this render as plain text.
const MAX_MARKDOWN_LENGTH = 50000;

const escapeHtml = (str: string): string => {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
};

const parsedContent = computed(() => {
  if (!props.message.content) return '';

  // While streaming: show plain text with basic formatting only.
  // marked.parse + DOMPurify are expensive; running them on every
  // chunk update causes the renderer to freeze.
  if (props.isReceiving) {
    return escapeHtml(props.message.content).replace(/\n/g, '<br>');
  }

  const rawContent = props.message.content;

  // Fallback for oversized messages.
  if (rawContent.length > MAX_MARKDOWN_LENGTH) {
    return `<pre class="whitespace-pre-wrap break-words text-sm">${escapeHtml(rawContent)}</pre>`;
  }

  try {
    const raw = marked.parse(rawContent) as string;
    return DOMPurify.sanitize(raw);
  } catch (e) {
    console.error('Markdown parse failed, falling back to plain text:', e);
    return `<pre class="whitespace-pre-wrap break-words text-sm">${escapeHtml(rawContent)}</pre>`;
  }
});

// Render mermaid when the component is mounted/updated and not actively streaming.
// We skip rendering during streaming to avoid flooding the renderer with
// partial diagram sources that may be syntactically invalid.
const shouldRenderMermaid = computed(() => !props.isReceiving);

watch(shouldRenderMermaid, (canRender) => {
  if (canRender) renderMermaid();
}, { immediate: true });

const copyCode = (event: MouseEvent) => {
  const target = event.target as HTMLElement;
  const btn = target.closest('.copy-btn') as HTMLElement | null;
  if (!btn) return;
  const pre = btn.closest('.relative')?.querySelector('pre');
  if (pre) {
    const code = pre.textContent || '';
    navigator.clipboard.writeText(code).then(() => {
      const originalText = btn.textContent;
      btn.textContent = 'Copied!';
      setTimeout(() => (btn.textContent = originalText), 1500);
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
    <div v-if="message.role === 'system'" class="text-[10px] th-text-muted px-3 py-1 th-active-bg rounded-full">
      {{ message.content }}
    </div>

    <!-- User message -->
    <div v-else-if="message.role === 'user'" class="flex items-end gap-2 max-w-[95%] md:max-w-[85%] lg:max-w-[75%]">
      <div class="flex flex-col items-end gap-0.5">
        <div class="px-4 py-2.5 rounded-2xl rounded-br-sm th-gradient-user text-white text-sm shadow-sm">
          <div class="whitespace-pre-wrap">{{ message.content }}</div>
        </div>
        <div class="flex items-center gap-1 px-1">
          <span class="text-[10px] th-text-dim">{{ formatTime(message.timestamp) }}</span>
          <Check v-if="message.status === 'sending'" class="w-3 h-3 th-text-dim" />
          <CheckCheck v-else-if="message.status === 'sent'" class="w-3 h-3 th-text-dim" />
          <div v-else-if="message.status === 'error'" class="flex items-center gap-1 text-destructive cursor-pointer hover:underline" @click="emit('retry')">
            <AlertCircle class="w-3 h-3" />
            <span class="text-[10px]">Failed</span>
          </div>
        </div>
      </div>
      <div class="w-7 h-7 rounded-full bg-muted flex items-center justify-center shrink-0">
        <User class="w-3.5 h-3.5 text-secondary-foreground" />
      </div>
    </div>

    <!-- Bot message -->
    <div v-else class="flex items-start gap-2 w-full">
      <div class="w-7 h-7 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center shrink-0 mt-0.5">
        <Bot class="w-3.5 h-3.5 text-white" />
      </div>
      <div class="flex flex-col gap-1 min-w-0 flex-1 pr-4">
        <div class="flex items-center gap-2">
          <span class="text-xs font-medium th-text-secondary">AI</span>
          <span class="text-[10px] th-text-dim">{{ formatTime(message.timestamp) }}</span>
        </div>

        <MessageThoughtsPanel
          :message="message"
          :is-thinking="isThinking"
          :is-last-bot-message="isLastBotMessage"
        />

        <!-- Content -->
        <div v-if="message.content || isStreaming"
          class="px-4 py-2.5 rounded-2xl rounded-tl-sm th-bubble-bg th-text text-sm border th-border shadow-sm min-w-0 w-full">
          <div ref="mermaidContainerRef" class="prose prose-invert prose-sm max-w-none" :data-md-style="markdownStyle" v-html="parsedContent" @click="copyCode" />
          <!-- Streaming cursor -->
          <span v-if="isStreaming" class="inline-block w-2 h-4 bg-primary/80 ml-0.5 align-middle animate-pulse rounded-sm" />
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
