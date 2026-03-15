<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Menu, MessageSquare, Pencil, Plus, Trash2, X } from 'lucide-vue-next';
import { onMounted, ref, watch } from 'vue';
import ChatArea from './components/ChatArea.vue';

// Types
export interface Message {
  id: string;
  role: 'user' | 'bot' | 'system';
  content: string;
  thinking?: string;
  toolCalls?: any[];
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

// Session rename state
const editingSessionId = ref<string | null>(null);
const editingName = ref('');

// Load from LocalStorage
onMounted(() => {
  const saved = localStorage.getItem('nanobot_sessions');
  if (saved) {
    try {
      sessions.value = JSON.parse(saved);
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
    localStorage.setItem('nanobot_sessions', JSON.stringify(sessions.value));
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
      { id: Date.now().toString(), role: 'system', content: 'Connected to Nanobot Gateway', timestamp: Date.now() }
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

const updateSessionMessages = (sessionId: string, messages: Message[]) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (session) {
    session.messages = messages;
    session.updatedAt = Date.now();
    // Update name based on first user message if still "New Chat"
    if (session.name === 'New Chat') {
      const firstUserMsg = messages.find(m => m.role === 'user');
      if (firstUserMsg && firstUserMsg.content) {
        session.name = firstUserMsg.content.slice(0, 30) + (firstUserMsg.content.length > 30 ? '...' : '');
      }
    }
  }
};

const clearSessionMessages = (sessionId: string) => {
  const session = sessions.value.find(s => s.id === sessionId);
  if (session) {
    session.messages = [];
    session.updatedAt = Date.now();
  }
};

const toggleSidebar = () => isSidebarOpen.value = !isSidebarOpen.value;
</script>

<template>
  <div class="flex h-screen w-full relative bg-slate-900 overflow-hidden">
    <!-- Mobile header for toggling sidebar -->
    <div class="md:hidden flex items-center gap-3 p-3 bg-slate-800/80 border-b border-white/10 absolute top-0 left-0 right-0 z-10 backdrop-blur-md">
      <Button variant="ghost" size="icon" @click="toggleSidebar" class="text-slate-100 hover:bg-white/10">
        <Menu />
      </Button>
      <h2 class="text-lg font-semibold bg-gradient-to-r from-blue-500 to-violet-500 bg-clip-text text-transparent">Nanobot</h2>
    </div>

    <!-- Sidebar -->
    <aside 
      class="fixed inset-y-0 left-0 md:relative z-20 flex w-72 flex-col bg-slate-900/95 backdrop-blur-xl border-r border-white/10 transition-transform duration-300 ease-in-out md:translate-x-0"
      :class="isSidebarOpen ? 'translate-x-0' : '-translate-x-full'"
    >
      <div class="p-5 flex justify-between items-center">
        <h1 class="text-xl font-semibold flex items-center gap-2.5">
          <div class="w-6 h-6 rounded-md bg-gradient-to-br from-blue-500 to-violet-500"></div>
          Nanobot AI
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
        @update-messages="(msgs) => updateSessionMessages(activeSessionId, msgs)"
        @clear-messages="clearSessionMessages(activeSessionId)"
      />
    </main>
  </div>
</template>

<style>
/* Any additional ultra custom overides can live here if needed, but Tailwind primarily powers it now */
</style>
