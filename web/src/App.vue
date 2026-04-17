<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { MessageSquare, Pencil, Plus, X, PanelLeftClose, PanelLeftOpen } from 'lucide-vue-next';
import { onMounted, onUnmounted, ref, watch } from 'vue';
import ChatArea from './components/ChatArea.vue';
import { useChatStore } from './stores/chatStore';
const chatStore = useChatStore();

// Sidebar width (persisted)
const MIN_WIDTH = 200;
const MAX_WIDTH = 480;
const DEFAULT_WIDTH = 280;
const COLLAPSED_WIDTH = 48;
const sidebarWidth = ref(DEFAULT_WIDTH);
const isCollapsed = ref(false);

onMounted(() => {
  const saved = localStorage.getItem('gasket_sidebar_width');
  if (saved) sidebarWidth.value = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, parseInt(saved, 10)));
  const savedCollapsed = localStorage.getItem('gasket_sidebar_collapsed');
  if (savedCollapsed) isCollapsed.value = savedCollapsed === 'true';
});

// Resize logic
const isResizing = ref(false);
const startX = ref(0);
const startWidth = ref(0);

const onResizeStart = (e: MouseEvent) => {
  if (isCollapsed.value) return;
  isResizing.value = true;
  startX.value = e.clientX;
  startWidth.value = sidebarWidth.value;
  document.body.style.cursor = 'col-resize';
  document.body.style.userSelect = 'none';
};

const onResizeMove = (e: MouseEvent) => {
  if (!isResizing.value) return;
  const delta = e.clientX - startX.value;
  const newWidth = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, startWidth.value + delta));
  sidebarWidth.value = newWidth;
};

const onResizeEnd = () => {
  if (!isResizing.value) return;
  isResizing.value = false;
  document.body.style.cursor = '';
  document.body.style.userSelect = '';
  localStorage.setItem('gasket_sidebar_width', String(sidebarWidth.value));
};

onMounted(() => {
  window.addEventListener('mousemove', onResizeMove);
  window.addEventListener('mouseup', onResizeEnd);
});

onUnmounted(() => {
  window.removeEventListener('mousemove', onResizeMove);
  window.removeEventListener('mouseup', onResizeEnd);
});

const toggleSidebar = () => {
  isCollapsed.value = !isCollapsed.value;
  localStorage.setItem('gasket_sidebar_collapsed', String(isCollapsed.value));
};

// Persist to localStorage
let saveTimer: ReturnType<typeof setTimeout> | null = null;
const debouncedSave = () => {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    localStorage.setItem('gasket_chats', JSON.stringify(chatStore.chats));
  }, 1000);
};
watch(() => chatStore.chats, debouncedSave, { deep: true });

const createNewChat = () => {
  chatStore.createChat();
};

const selectChat = (id: string) => {
  chatStore.setActiveChat(id);
};

const startRename = (chatId: string, currentName: string, event: Event) => {
  event.stopPropagation();
  editingChatId.value = chatId;
  editingName.value = currentName;
};

const confirmRename = (chatId: string) => {
  if (editingName.value.trim()) {
    chatStore.renameChat(chatId, editingName.value.trim());
  }
  editingChatId.value = null;
};

const cancelRename = () => {
  editingChatId.value = null;
};

const handleRenameKeydown = (event: KeyboardEvent, chatId: string) => {
  if (event.key === 'Enter') {
    confirmRename(chatId);
  } else if (event.key === 'Escape') {
    cancelRename();
  }
};

// Session rename state
const editingChatId = ref<string | null>(null);
const editingName = ref('');
const connectionStatus = ref<boolean>(false);

onUnmounted(() => {
  if (saveTimer) clearTimeout(saveTimer);
});
</script>

<template>
  <div class="flex h-screen w-full bg-gray-50 dark:bg-slate-900 overflow-hidden">
    <!-- Sidebar Drawer -->
    <aside
      class="relative flex flex-col bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl border-r border-gray-200 dark:border-white/10 shrink-0 transition-all duration-300 ease-in-out"
      :class="isCollapsed ? 'items-center overflow-hidden' : ''"
      :style="{ width: (isCollapsed ? COLLAPSED_WIDTH : sidebarWidth) + 'px' }"
    >
      <!-- Collapsed view -->
      <template v-if="isCollapsed">
        <div class="flex flex-col items-center gap-3 py-3 h-full">
          <button
            class="w-8 h-8 rounded-lg bg-gradient-to-br from-blue-500 to-violet-500 flex items-center justify-center text-white hover:opacity-90 transition-opacity"
            @click="createNewChat"
            title="New Chat"
          >
            <Plus class="w-4 h-4" />
          </button>

          <div class="flex flex-col items-center gap-1.5 overflow-y-auto flex-1 min-h-0">
            <button
              v-for="chat in chatStore.chats"
              :key="chat.id"
              class="w-8 h-8 rounded-lg flex items-center justify-center text-xs font-bold transition-all"
              :class="chat.id === chatStore.activeChatId
                ? 'bg-blue-50 dark:bg-blue-500/20 text-blue-600 dark:text-blue-400'
                : 'text-gray-500 dark:text-slate-400 hover:bg-gray-100 dark:hover:bg-white/5'"
              @click="selectChat(chat.id)"
              :title="chat.name"
            >
              {{ chat.name.charAt(0).toUpperCase() }}
            </button>
          </div>

          <button
            class="w-8 h-8 rounded-lg flex items-center justify-center text-gray-500 dark:text-slate-400 hover:bg-gray-100 dark:hover:bg-white/10 hover:text-gray-800 dark:hover:text-slate-200 transition-colors"
            @click="toggleSidebar"
            title="Expand sidebar"
          >
            <PanelLeftOpen class="w-4 h-4" />
          </button>
        </div>
      </template>

      <!-- Expanded view -->
      <template v-else>
        <div class="p-4 flex justify-between items-center border-b border-gray-200 dark:border-white/5">
          <h1 class="text-lg font-semibold flex items-center gap-2.5 text-gray-900 dark:text-slate-100">
            <div class="w-7 h-7 rounded-lg bg-gradient-to-br from-blue-500 to-violet-500 flex items-center justify-center">
              <MessageSquare class="w-4 h-4 text-white" />
            </div>
            Chats
          </h1>
          <Button variant="ghost" size="icon" class="text-gray-500 dark:text-slate-400 hover:text-gray-800 dark:hover:text-slate-200" @click="toggleSidebar">
            <PanelLeftClose class="w-4 h-4" />
          </Button>
        </div>

        <ScrollArea class="flex-1">
          <div class="flex flex-col gap-0.5 p-2">
            <div
              v-for="chat in chatStore.chats"
              :key="chat.id"
              class="group flex items-center gap-3 px-3 py-2 rounded-xl cursor-pointer text-gray-500 dark:text-slate-400 transition-all duration-200 hover:bg-gray-100 dark:hover:bg-white/5 hover:text-gray-800 dark:hover:text-slate-100 relative"
              :class="{ 'bg-gray-100 dark:bg-slate-800/60 text-gray-900 dark:text-slate-100': chat.id === chatStore.activeChatId }"
              @click="selectChat(chat.id)"
            >
              <div class="flex-1 min-w-0">
                <div class="flex items-center justify-between">
                  <template v-if="editingChatId === chat.id">
                    <input
                      v-model="editingName"
                      @click.stop
                      @keydown="(e) => handleRenameKeydown(e, chat.id)"
                      @blur="confirmRename(chat.id)"
                      class="flex-1 text-sm bg-white dark:bg-slate-800 border border-blue-500/50 rounded px-1.5 py-0.5 text-gray-900 dark:text-slate-100 outline-none focus:ring-1 focus:ring-blue-500/30 min-w-0"
                      autofocus
                    />
                  </template>
                  <template v-else>
                    <span class="text-sm font-medium text-gray-800 dark:text-slate-200 truncate">{{ chat.name }}</span>
                  </template>
                </div>
                  <span class="text-[10px] text-gray-400 dark:text-slate-500 mt-0.5">
                    {{ chat.messages[chat.messages.length - 1]?.timestamp ? new Date(chat.messages[chat.messages.length - 1].timestamp).toLocaleDateString() : '' }}
                  </span>
              </div>

              <div class="opacity-0 group-hover:opacity-100 transition-opacity flex items-center gap-0.5">
                <button
                  @click.stop="startRename(chat.id, chat.name, $event)"
                  class="p-1 rounded hover:bg-gray-200 dark:hover:bg-white/10 text-gray-400 dark:text-slate-500 hover:text-gray-600 dark:hover:text-slate-300"
                  title="Rename"
                >
                  <Pencil class="w-3 h-3" />
                </button>
                <button
                  @click.stop="chatStore.deleteChat(chat.id)"
                  class="p-1 rounded hover:bg-red-100 dark:hover:bg-red-500/10 text-gray-400 dark:text-slate-500 hover:text-red-500 dark:hover:text-red-400"
                  title="Delete"
                >
                  <X class="w-3 h-3" />
                </button>
              </div>
            </div>
          </div>
        </ScrollArea>

        <div class="p-3 border-t border-gray-200 dark:border-white/5">
          <Button variant="outline" class="w-full justify-start gap-2 bg-gray-50 dark:bg-white/5 border-gray-200 dark:border-white/10 hover:bg-gray-100 dark:hover:bg-white/10 text-gray-900 dark:text-slate-100" @click="createNewChat">
            <Plus class="w-4 h-4" />
            New Chat
          </Button>
        </div>
      </template>

      <!-- Resize handle -->
      <div
        v-if="!isCollapsed"
        class="absolute top-0 right-0 bottom-0 w-1 cursor-col-resize z-20 hover:bg-blue-500/30 transition-colors"
        :class="isResizing ? 'bg-blue-500/40' : 'bg-transparent'"
        @mousedown="onResizeStart"
      />
    </aside>

    <!-- Main Chat Area -->
    <main class="flex-1 flex flex-col min-w-0 relative bg-white dark:bg-slate-800/40">
      <ChatArea
        v-if="chatStore.activeChatId"
        :chat-id="chatStore.activeChatId"
        @connection-status="connectionStatus = $event"
      />
    </main>
  </div>
</template>
