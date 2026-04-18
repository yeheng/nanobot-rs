import { computed, nextTick, reactive, ref } from 'vue';
import { useChatStore } from '@/stores/chatStore';
import { useIMWebSocket } from '@/hooks/useIMWebSocket';
import type { Message, SubagentState } from '@/types';

export function useChatSession(chatId: { value: string }) {
  const chatStore = useChatStore();

  const isThinking = ref(false);
  const isSending = ref(false);
  const isReceiving = ref(false);
  const isCompacting = ref(false);

  const toolStartTimes = ref<Record<string, number>>({});
  const activeSubagents = ref<Map<string, SubagentState>>(new Map());
  const hasActiveSubagents = computed(() => activeSubagents.value.size > 0);

  const errorBanner = ref<string | null>(null);
  let errorBannerTimer: ReturnType<typeof setTimeout> | null = null;

  const contextStats = computed(() => chatStore.activeChat?.contextStats);
  const watermarkInfo = computed(() => chatStore.activeChat?.watermarkInfo);

  const usageColor = computed(() => {
    const pct = contextStats.value?.usage_percent || 0;
    if (pct < 80) return 'bg-primary';
    if (pct < 100) return 'bg-amber-500';
    return 'bg-destructive';
  });

  type SessionStatus = 'disconnected' | 'idle' | 'sending' | 'receiving';

  const showError = (message: string) => {
    errorBanner.value = message;
    if (errorBannerTimer) clearTimeout(errorBannerTimer);
    errorBannerTimer = setTimeout(() => { errorBanner.value = null; }, 8000);
  };

  const dismissError = () => {
    errorBanner.value = null;
    if (errorBannerTimer) clearTimeout(errorBannerTimer);
  };

  // ── Subagent handling ───────────────────────────────────────

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

  const generateSubagentSummary = (subagents: SubagentState[]): string => {
    const lines = ['\n\n---\n**✅ 并行任务完成**\n'];
    for (const s of subagents) {
      const status = s.status === 'error' ? '❌' : '✓';
      const duration = s.endTime ? ((s.endTime - s.startTime) / 1000).toFixed(1) : '?';
      lines.push(`${status} **Task ${s.index}**: ${s.summary || s.task} _(${s.toolCount} 工具, ${duration}s)_`);
    }
    return lines.join('\n');
  };

  const finalizeSubagents = (botMsg: Message) => {
    const subagents = [...activeSubagents.value.values()].sort((a, b) => a.index - b.index);
    if (subagents.length === 0) return;
    const summary = generateSubagentSummary(subagents);
    chatStore.appendToMessage(chatId.value, botMsg.id, summary, 'content');
    activeSubagents.value.clear();
  };

  const checkAndFinalizeSubagents = (botMsg: Message) => {
    const allCompleted = [...activeSubagents.value.values()].every(s => s.status !== 'running');
    if (allCompleted && activeSubagents.value.size > 0) {
      finalizeSubagents(botMsg);
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

  // ── WebSocket message processing ────────────────────────────

  const processWebSocketMessageInner = (msg: any, botMsg: Message) => {
    switch (msg.type) {
      case 'thinking':
        isThinking.value = true;
        chatStore.appendToMessage(chatId.value, botMsg.id, msg.content, 'thinking');
        break;
      case 'tool_start':
        isThinking.value = true;
        chatStore.ensureToolCalls(chatId.value, botMsg.id);
        const toolId = Date.now().toString() + '_' + Math.random().toString(36).substr(2, 9);
        chatStore.pushToolCall(chatId.value, botMsg.id, {
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
        const toolCalls = chatStore.activeMessages.find(m => m.id === botMsg.id)?.toolCalls;
        if (toolCalls && toolCalls.length > 0) {
          const matchingTool = [...toolCalls].reverse().find(t => t.name === msg.name && t.status === 'running');
          const activeTool = matchingTool || [...toolCalls].reverse().find(t => t.status === 'running') || toolCalls[toolCalls.length - 1];
          const updates: any = { status: msg.error ? 'error' : 'complete', result: msg.error || msg.output };
          if (toolStartTimes.value[activeTool.id]) {
            updates.duration = ((Date.now() - toolStartTimes.value[activeTool.id]) / 1000).toFixed(1);
            delete toolStartTimes.value[activeTool.id];
          }
          chatStore.updateToolCall(chatId.value, botMsg.id, activeTool.id, updates);
        }
        break;
      case 'content':
      case 'text':
        isThinking.value = false;
        chatStore.appendToMessage(chatId.value, botMsg.id, msg.content, 'content');
        break;
      case 'error':
        isThinking.value = false;
        showError(msg.content || msg.message || 'An error occurred');
        break;
      case 'done':
        isThinking.value = false;
        isReceiving.value = false;
        fetchContext();
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

  const processWebSocketMessage = (msg: any) => {
    isSending.value = false;
    isReceiving.value = true;

    let botMsg = chatStore.activeMessages[chatStore.activeMessages.length - 1];
    if (!botMsg || botMsg.role !== 'bot') {
      chatStore.appendMessage(chatId.value, {
        id: Date.now().toString(),
        role: 'bot',
        content: '',
        timestamp: Date.now()
      });
      nextTick(() => {
        botMsg = chatStore.activeMessages[chatStore.activeMessages.length - 1];
        processWebSocketMessageInner(msg, botMsg);
      });
      return;
    }

    processWebSocketMessageInner(msg, botMsg);
  };

  const handleMessage = (data: string) => {
    try {
      const msg = JSON.parse(data);
      processWebSocketMessage(msg);
    } catch (e) {
      isThinking.value = false;
      isSending.value = false;
      const lastMsg = chatStore.activeMessages[chatStore.activeMessages.length - 1];
      if (lastMsg && lastMsg.role === 'bot') {
        chatStore.appendToMessage(chatId.value, lastMsg.id, data, 'content');
      }
    }
  };

  const { isConnected, showReconnectButton, connect, manualReconnect, send } =
    useIMWebSocket(computed(() => chatId.value), handleMessage);

  const sessionStatus = computed<SessionStatus>(() => {
    if (!isConnected.value) return 'disconnected';
    if (isSending.value) return 'sending';
    if (isReceiving.value || isThinking.value) return 'receiving';
    return 'idle';
  });

  // ── Context API ─────────────────────────────────────────────

  const baseUrl = () => import.meta.env.VITE_API_URL || 'http://localhost:3000';
  const sessionKey = () => encodeURIComponent(`websocket:${chatId.value}`);

  const fetchContext = async () => {
    try {
      const res = await fetch(`${baseUrl()}/api/sessions/${sessionKey()}/context`);
      const data = await res.json();
      if (res.ok && data.context_stats) {
        chatStore.setContextStats(chatId.value, data.context_stats);
      }
      if (res.ok && data.watermark_info) {
        chatStore.setWatermarkInfo(chatId.value, data.watermark_info);
      }
    } catch (e) {
      console.error('Fetch context failed:', e);
    }
  };

  const forceCompact = async () => {
    if (isCompacting.value) return;
    isCompacting.value = true;
    try {
      const res = await fetch(`${baseUrl()}/api/sessions/${sessionKey()}/context/compact`, { method: 'POST' });
      const data = await res.json();
      if (res.ok && data.context_stats) {
        chatStore.setContextStats(chatId.value, data.context_stats);
      }
      if (res.ok && data.watermark_info) {
        chatStore.setWatermarkInfo(chatId.value, data.watermark_info);
      }
    } catch (e) {
      console.error('Force compact failed:', e);
    } finally {
      isCompacting.value = false;
    }
  };

  // ── Public interface ────────────────────────────────────────

  const stopGenerating = () => {
    send(JSON.stringify({ type: 'cancel' }));
    isThinking.value = false;
    isReceiving.value = false;
    isSending.value = false;
    chatStore.abortToolCalls(chatId.value);
  };

  const sendMessage = (text: string) => {
    if (!text.trim() || !isConnected.value || isSending.value || isReceiving.value) return false;

    const msgId = Date.now().toString();
    chatStore.appendMessage(chatId.value, {
      id: msgId,
      role: 'user',
      content: text,
      timestamp: Date.now(),
      status: 'sending'
    });

    isSending.value = true;
    try {
      send(text);
      chatStore.updateMessageStatus(chatId.value, msgId, 'sent');
      return true;
    } catch (e) {
      chatStore.updateMessageStatus(chatId.value, msgId, 'error');
      return false;
    }
  };

  const retryMessage = (msgId: string, content: string) => {
    if (!isConnected.value) return;
    chatStore.updateMessageStatus(chatId.value, msgId, 'sending');
    try {
      send(content);
      chatStore.updateMessageStatus(chatId.value, msgId, 'sent');
    } catch (e) {
      chatStore.updateMessageStatus(chatId.value, msgId, 'error');
    }
  };

  return reactive({
    // Status
    isConnected,
    isThinking,
    isSending,
    isReceiving,
    isCompacting,
    sessionStatus,
    showReconnectButton,
    // Context
    contextStats,
    watermarkInfo,
    usageColor,
    // Subagents
    activeSubagents,
    hasActiveSubagents,
    // Error
    errorBanner,
    // Actions
    connect,
    manualReconnect,
    sendMessage,
    retryMessage,
    stopGenerating,
    fetchContext,
    forceCompact,
    dismissError,
  });
}
