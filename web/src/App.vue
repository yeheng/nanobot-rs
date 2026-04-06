<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Menu, MessageSquare, Pencil, Plus, Trash2, X } from 'lucide-vue-next';
import { onMounted, onUnmounted, ref, watch } from 'vue';
import ChatArea from './components/ChatArea.vue';

// Types
export interface ToolCall {
  id: string;
  name: string;
  arguments?: string;
  status: 'running' | 'complete' | 'error';
  result?: string | null;
  duration?: string;
}

export interface Message {
  id: string;
  role: 'user' | 'bot' | 'system';
  content: string;
  thinking?: string;
  toolCalls?: ToolCall[];
  steps?: any[];
  timestamp: number;
}

export interface Session {
  id: string;
  name: string;
  messages: Message[];
  updatedAt: number;
}

// State
const sessions = ref<Session[]>([]);
const activeSessionId = ref<string>('');
const isSidebarOpen = ref(true);

// --- Data Migration: Convert steps to simple fields ---
/** Migrate legacy steps format to simple thinking/toolCalls/content fields */
const migrateLegacyMessage = (msg: Message): Message => {
  // If no steps, already in correct format
  if (!msg.steps || msg.steps.length === 0) {
    return msg;
  }

  let thinking = msg.thinking || '';
  let toolCalls = msg.toolCalls || [];
  let content = msg.content || '';

  // Extract data from steps
  for (const step of msg.steps) {
    if (step.type === 'thinking' && step.content) {
      thinking += step.content;
    } else if (step.type === 'tool_group' && step.tools) {
      toolCalls = [...toolCalls, ...step.tools];
    } else if (step.type === 'content' && step.content) {
      content += step.content;
    }
  }

  // Return message without steps (cleaner format)
  const { steps, ...rest } = msg;
  return {
    ...rest,
    thinking: thinking || undefined,
    toolCalls: toolCalls.length > 0 ? toolCalls : undefined,
    content: content
  };
};

/** Migrate all sessions' messages on load */
const migrateSessionsData = (loadedSessions: Session[]): Session[] => {
  return loadedSessions.map(session => ({
    ...session,
    messages: session.messages.map(migrateLegacyMessage)
  }));
};

// Session rename state
const editingSessionId = ref<string | null>(null);
const editingName = ref('');

// Load from LocalStorage with data migration
onMounted(() => {
  const saved = localStorage.getItem('gasket_sessions');
  if (saved) {
    try {
      const loadedSessions = JSON.parse(saved);
      // Migrate legacy message format to steps-based format
      sessions.value = migrateSessionsData(loadedSessions);
      if (sessions.value && sessions.value.length > 0) {
        activeSessionId.value = sessions.value[0].id;
      }
    } catch (e) {
      console.error('Failed to parse sessions from local storage:', e);
    }
  }

  // Create first session if empty
  if (sessions.value.length === 0) {
    createNewSession();
  }
});

// Debounced save to LocalStorage
let saveTimer: ReturnType<typeof setTimeout> | null = null;
const debouncedSave = () => {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    localStorage.setItem('gasket_sessions', JSON.stringify(sessions.value));
  }, 1000);
};

watch(sessions, () => {
  debouncedSave();
}, { deep: true });

const createNewSession = () => {
  const newSession: Session = {
    id: 'session_' + Date.now() + '_' + Math.random().toString(36).substr(2, 9),
    name: `New Chat`,
    messages: [
      { id: Date.now().toString(), role: 'system', content: 'Connected to gasket Gateway', timestamp: Date.now() }
    ],
    updatedAt: Date.now()
  };
  sessions.value.unshift(newSession);
  activeSessionId.value = newSession.id;
  
  if (window.innerWidth < 768) {
    isSidebarOpen.value = false;
  }
};

const deleteSession = (id: string) => {
  sessions.value = sessions.value.filter(s => s.id !== id);
  if (activeSessionId.value === id) {
    activeSessionId.value = (sessions.value && sessions.value.length > 0) ? sessions.value[0].id : '';
  }
  if (sessions.value.length === 0) {
    createNewSession();
  }
};

const selectSession = (id: string) => {
  activeSessionId.value = id;
  if (window.innerWidth < 768) {
    isSidebarOpen.value = false;
  }
};

const deleteAllSessions = () => {
  if (confirm('Are you sure you want to delete all chats? This cannot be undone.')) {
    sessions.value = [];
    createNewSession();
  }
};

// Session rename
const startRename = (session: Session, event: Event) => {
  event.stopPropagation();
  editingSessionId.value = session.id;
  editingName.value = session.name;
};

const confirmRename = (sessionId: string) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (session && editingName.value.trim()) {
    session.name = editingName.value.trim();
  }
  editingSessionId.value = null;
};

const cancelRename = () => {
  editingSessionId.value = null;
};

const handleRenameKeydown = (event: KeyboardEvent, sessionId: string) => {
  if (event.key === 'Enter') {
    confirmRename(sessionId);
  } else if (event.key === 'Escape') {
    cancelRename();
  }
};

// --- Incremental message update methods (no deep copy) ---

/** Get or create the latest bot message for streaming responses */
const getOrCreateLatestBotMessage = (sessionId: string): Message | null => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (!session) return null;

  const lastMsg = session.messages[session.messages.length - 1];
  if (lastMsg && lastMsg.role === 'bot') {
    return lastMsg;
  }

  const newBotMsg: Message = {
    id: Date.now().toString(),
    role: 'bot',
    content: '',
    timestamp: Date.now()
  };
  session.messages.push(newBotMsg);
  return newBotMsg;
};

/** Append a new message (user or system) */
const appendMessage = (sessionId: string, message: Message) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (session) {
    session.messages.push(message);
    session.updatedAt = Date.now();
    // Update name based on first user message (sanitized)
    if (session.name === 'New Chat' && message.role === 'user' && message.content) {
      // Sanitize: remove control characters, trim whitespace, limit length
      const sanitizedName = message.content
        .replace(/[\u0000-\u001F\u007F]/g, '') // Remove control characters
        .replace(/\s+/g, ' ') // Normalize whitespace
        .trim()
        .slice(0, 50); // Increased to 50 chars for better context
      session.name = sanitizedName + (message.content.length > 50 ? '...' : '');
    }
  }
};

/** Append content to a specific message field (for streaming) */
const appendToMessage = (sessionId: string, messageId: string, content: string, field: 'content' | 'thinking' = 'content') => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (!session) return;
  const msg = session.messages.find(m => m.id === messageId);
  if (msg) {
    if (field === 'thinking') {
      msg.thinking = (msg.thinking || '') + content;
    } else {
      msg.content = (msg.content || '') + content;
    }
    session.updatedAt = Date.now();
  }
};

/** Update message with partial data (tool calls, steps, etc.) */
const updateMessage = (sessionId: string, messageId: string, updates: Partial<Message>) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (!session) return;
  const msg = session.messages.find(m => m.id === messageId);
  if (msg) {
    Object.assign(msg, updates);
    session.updatedAt = Date.now();
  }
};

/** Ensure tool calls array exists */
const ensureMessageToolCalls = (sessionId: string, messageId: string) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (!session) return;
  const msg = session.messages.find(m => m.id === messageId);
  if (msg && !msg.toolCalls) {
    msg.toolCalls = [];
  }
};

/** Push a tool call */
const pushToolCall = (sessionId: string, messageId: string, toolCall: any) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (!session) return;
  const msg = session.messages.find(m => m.id === messageId);
  if (msg) {
    if (!msg.toolCalls) msg.toolCalls = [];
    msg.toolCalls.push(toolCall);
    session.updatedAt = Date.now();
  }
};

/** Update a specific tool call by id */
const updateToolCall = (sessionId: string, messageId: string, toolId: string, updates: any) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (!session) return;
  const msg = session.messages.find(m => m.id === messageId);
  if (msg && msg.toolCalls) {
    const tool = msg.toolCalls.find(t => t.id === toolId);
    if (tool) {
      Object.assign(tool, updates);
      session.updatedAt = Date.now();
    }
  }
};

/** Clear all messages for a session */
const clearSessionMessages = (sessionId: string) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (session) {
    session.messages = [];
    session.updatedAt = Date.now();
  }
};

const toggleSidebar = () => isSidebarOpen.value = !isSidebarOpen.value;

// Cleanup on unmount
onUnmounted(() => {
  if (saveTimer) clearTimeout(saveTimer);
});
</script>

<template>
  <div class="flex h-screen w-full relative bg-slate-900 overflow-hidden">
    <!-- Mobile header for toggling sidebar -->
    <div class="md:hidden flex items-center gap-3 p-3 bg-slate-800/80 border-b border-white/10 absolute top-0 left-0 right-0 z-10 backdrop-blur-md">
      <Button variant="ghost" size="icon" @click="toggleSidebar" class="text-slate-100 hover:bg-white/10">
        <Menu />
      </Button>
      <h2 class="text-lg font-semibold bg-gradient-to-r from-blue-500 to-violet-500 bg-clip-text text-transparent">gasket</h2>
    </div>

    <!-- Sidebar -->
    <aside 
      class="fixed inset-y-0 left-0 md:relative z-20 flex w-72 flex-col bg-slate-900/95 backdrop-blur-xl border-r border-white/10 transition-transform duration-300 ease-in-out md:translate-x-0"
      :class="isSidebarOpen ? 'translate-x-0' : '-translate-x-full'"
    >
      <div class="p-5 flex justify-between items-center">
        <h1 class="text-xl font-semibold flex items-center gap-2.5">
          <div class="w-6 h-6 rounded-md bg-gradient-to-br from-blue-500 to-violet-500"></div>
          gasket AI
        </h1>
        <Button variant="ghost" size="icon" class="md:hidden text-slate-400" @click="toggleSidebar">
          <X class="w-5 h-5"/>
        </Button>
      </div>
      
      <div class="px-4 pb-4 flex gap-2">
        <Button variant="outline" class="flex-1 justify-start gap-2 bg-white/5 border-white/10 hover:bg-white/10 hover:border-white/20 text-slate-100" @click="createNewSession">
          <Plus class="w-4 h-4" />
          New Chat
        </Button>
        <Button variant="outline" size="icon" class="bg-white/5 border-white/10 hover:bg-red-500/20 hover:text-red-400 hover:border-red-500/30 text-slate-400" @click="deleteAllSessions" title="Delete all chats">
          <Trash2 class="w-4 h-4" />
        </Button>
      </div>

      <ScrollArea class="flex-1 px-4 pb-4">
        <div class="flex flex-col gap-1">
          <div 
            v-for="session in sessions" 
            :key="session.id"
            class="group flex items-center gap-3 p-3 rounded-lg cursor-pointer text-slate-400 transition-all duration-200 hover:bg-white/5 hover:text-slate-100 relative"
            :class="{ 'bg-blue-500/15 text-blue-400 hover:text-blue-400': session.id === activeSessionId }"
            @click="selectSession(session.id)"
          >
            <MessageSquare class="w-4 h-4 shrink-0 opacity-70 group-hover:opacity-100" />
            
            <!-- Editing mode -->
            <template v-if="editingSessionId === session.id">
              <input
                v-model="editingName"
                @click.stop
                @keydown="(e) => handleRenameKeydown(e, session.id)"
                @blur="confirmRename(session.id)"
                class="flex-1 text-sm bg-slate-800 border border-blue-500/50 rounded px-1.5 py-0.5 text-slate-100 outline-none focus:ring-1 focus:ring-blue-500/30 min-w-0"
                autofocus
              />
            </template>
            <!-- Display mode -->
            <template v-else>
              <span class="flex-1 truncate text-sm" @dblclick="startRename(session, $event)">{{ session.name }}</span>
            </template>

            <div class="flex items-center gap-0.5 opacity-30 group-hover:opacity-100 transition-opacity">
              <button 
                class="items-center justify-center p-1 text-slate-400 hover:text-blue-400 transition-colors"
                @click.stop="startRename(session, $event)"
                title="Rename"
              >
                <Pencil class="w-3.5 h-3.5" />
              </button>
              <button 
                class="items-center justify-center p-1 text-slate-400 hover:text-red-500 transition-colors"
                @click.stop="deleteSession(session.id)"
                title="Delete"
              >
                <Trash2 class="w-4 h-4" />
              </button>
            </div>
          </div>
        </div>
      </ScrollArea>
    </aside>

    <!-- Overlay for mobile -->
    <div 
      v-if="isSidebarOpen" 
      class="fixed inset-0 bg-black/50 z-15 md:hidden"
      @click="toggleSidebar"
    ></div>

    <!-- Main Chat Area -->
    <main class="flex-1 flex flex-col min-w-0 relative bg-slate-800/40 md:pt-0 pt-14">
      <ChatArea
        v-if="activeSessionId"
        :session-id="activeSessionId"
        :messages="sessions.find(s => s.id === activeSessionId)?.messages || []"
        @get-or-create-bot-msg="() => getOrCreateLatestBotMessage(activeSessionId)"
        @append-message="(msg) => appendMessage(activeSessionId, msg)"
        @append-to-message="(msgId, content, field) => appendToMessage(activeSessionId, msgId, content, field)"
        @update-message="(msgId, updates) => updateMessage(activeSessionId, msgId, updates)"
        @ensure-tool-calls="(msgId) => ensureMessageToolCalls(activeSessionId, msgId)"
        @push-tool-call="(msgId, tool) => pushToolCall(activeSessionId, msgId, tool)"
        @update-tool-call="(msgId, toolId, updates) => updateToolCall(activeSessionId, msgId, toolId, updates)"
        @clear-messages="clearSessionMessages(activeSessionId)"
      />
    </main>
  </div>
</template>

<style>
/* Any additional ultra custom overides can live here if needed, but Tailwind primarily powers it now */
</style>
