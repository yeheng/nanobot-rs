<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { MessageSquare, Plus, Pencil, Sun, Moon, X } from 'lucide-vue-next';
import { onUnmounted, ref, watch } from 'vue';
import ChatArea from './components/ChatArea.vue';
import { useChatStore } from './stores/chatStore';
import { useTheme } from './composables/useTheme';

const chatStore = useChatStore();
const { theme, toggle } = useTheme();

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
    <!-- Main Chat Area -->
    <main class="flex-1 flex flex-col min-w-0 relative bg-white dark:bg-slate-800/40">
      <!-- Header with Tabs -->
      <header class="shrink-0 bg-white/80 dark:bg-slate-800/80 border-b border-gray-200 dark:border-white/10 backdrop-blur-md z-10">
        <div class="flex items-center justify-between px-4 py-2.5">
          <!-- Logo -->
          <div class="flex items-center gap-2.5 shrink-0">
            <div class="w-8 h-8 rounded-lg bg-gradient-to-br from-blue-500 to-violet-500 flex items-center justify-center">
              <MessageSquare class="w-4 h-4 text-white" />
            </div>
            <h1 class="text-base font-semibold bg-gradient-to-r from-blue-500 to-violet-500 bg-clip-text text-transparent">Chats</h1>
          </div>

          <div class="flex items-center gap-1.5">
            <Button variant="ghost" size="icon" class="text-gray-500 dark:text-slate-400 hover:text-gray-800 dark:hover:text-slate-200" @click="toggle">
              <Sun v-if="theme === 'dark'" class="w-4 h-4" />
              <Moon v-else class="w-4 h-4" />
            </Button>
          </div>
        </div>

        <!-- Horizontal Tabs -->
        <div class="px-3 pb-2">
          <ScrollArea class="w-full">
            <div class="flex items-center gap-1">
              <button
                v-for="chat in chatStore.chats"
                :key="chat.id"
                class="group flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all shrink-0 border"
                :class="[
                  chat.id === chatStore.activeChatId
                    ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-200 dark:border-blue-500/20'
                    : 'bg-gray-50 dark:bg-slate-700/30 text-gray-600 dark:text-slate-400 border-gray-200 dark:border-white/5 hover:bg-gray-100 dark:hover:bg-white/5 hover:text-gray-800 dark:hover:text-slate-200'
                ]"
                @click="selectChat(chat.id)"
              >
                <!-- Editable name -->
                <template v-if="editingChatId === chat.id">
                  <input
                    v-model="editingName"
                    @click.stop
                    @keydown="(e) => handleRenameKeydown(e, chat.id)"
                    @blur="confirmRename(chat.id)"
                    class="w-24 bg-white dark:bg-slate-800 border border-blue-500/50 rounded px-1 py-0.5 text-xs text-gray-900 dark:text-slate-100 outline-none focus:ring-1 focus:ring-blue-500/30"
                    autofocus
                  />
                </template>
                <template v-else>
                  <span class="truncate max-w-[120px]">{{ chat.name }}</span>
                </template>

                <!-- Actions -->
                <div
                  v-if="editingChatId !== chat.id"
                  class="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity"
                >
                  <button
                    @click.stop="startRename(chat.id, chat.name, $event)"
                    class="p-0.5 rounded hover:bg-gray-200 dark:hover:bg-white/10 text-gray-400 dark:text-slate-500 hover:text-gray-600 dark:hover:text-slate-300"
                    title="Rename"
                  >
                    <Pencil class="w-3 h-3" />
                  </button>
                  <button
                    @click.stop="chatStore.deleteChat(chat.id)"
                    class="p-0.5 rounded hover:bg-red-100 dark:hover:bg-red-500/10 text-gray-400 dark:text-slate-500 hover:text-red-500 dark:hover:text-red-400"
                    title="Delete"
                  >
                    <X class="w-3 h-3" />
                  </button>
                </div>
              </button>

              <!-- New Chat Button -->
              <button
                @click="createNewChat"
                class="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium border border-dashed border-gray-300 dark:border-white/10 text-gray-500 dark:text-slate-400 hover:bg-gray-50 dark:hover:bg-white/5 hover:border-gray-400 dark:hover:border-white/20 transition-all shrink-0"
              >
                <Plus class="w-3.5 h-3.5" />
                New
              </button>
            </div>
          </ScrollArea>
        </div>
      </header>

      <ChatArea
        v-if="chatStore.activeChatId"
        :chat-id="chatStore.activeChatId"
        @connection-status="connectionStatus = $event"
      />
    </main>
  </div>
</template>
