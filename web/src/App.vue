<script setup lang="ts">
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Menu, MessageSquare, Pencil, Plus, Trash2, X, MoreVertical, Bot, Sun, Moon } from 'lucide-vue-next';
import { onMounted, onUnmounted, ref, watch } from 'vue';
import { Menu as HeadlessMenu, MenuButton, MenuItems, MenuItem } from '@headlessui/vue';
import ChatArea from './components/ChatArea.vue';
import { useChatStore } from './stores/chatStore';
import { useTheme } from './composables/useTheme';

const chatStore = useChatStore();
const { theme, toggle } = useTheme();
const isSidebarOpen = ref(true);

// Persist to localStorage
let saveTimer: ReturnType<typeof setTimeout> | null = null;
const debouncedSave = () => {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    localStorage.setItem('gasket_chats', JSON.stringify(chatStore.chats));
  }, 1000);
};
watch(() => chatStore.chats, debouncedSave, { deep: true });

// Session rename state
const editingChatId = ref<string | null>(null);
const editingName = ref('');

onMounted(() => {
  if (window.innerWidth < 768) {
    isSidebarOpen.value = false;
  }
});

const createNewChat = () => {
  chatStore.createChat();
  if (window.innerWidth < 768) {
    isSidebarOpen.value = false;
  }
};

const selectChat = (id: string) => {
  chatStore.setActiveChat(id);
  if (window.innerWidth < 768) {
    isSidebarOpen.value = false;
  }
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

const toggleSidebar = () => isSidebarOpen.value = !isSidebarOpen.value;

onUnmounted(() => {
  if (saveTimer) clearTimeout(saveTimer);
});
</script>

<template>
  <div class="flex h-screen w-full relative bg-gray-50 dark:bg-slate-900 overflow-hidden">
    <!-- Mobile header for toggling sidebar -->
    <div class="md:hidden flex items-center gap-3 p-3 bg-white/80 dark:bg-slate-800/80 border-b border-gray-200 dark:border-white/10 absolute top-0 left-0 right-0 z-10 backdrop-blur-md">
      <Button variant="ghost" size="icon" @click="toggleSidebar" class="text-gray-800 dark:text-slate-100 hover:bg-gray-200 dark:hover:bg-white/10">
        <Menu />
      </Button>
      <h2 class="text-lg font-semibold bg-gradient-to-r from-blue-500 to-violet-500 bg-clip-text text-transparent">Chats</h2>
    </div>

    <!-- Sidebar -->
    <aside 
      class="fixed inset-y-0 left-0 md:relative z-20 flex w-80 flex-col bg-white/95 dark:bg-slate-900/95 backdrop-blur-xl border-r border-gray-200 dark:border-white/10 transition-transform duration-300 ease-in-out md:translate-x-0"
      :class="isSidebarOpen ? 'translate-x-0' : '-translate-x-full'"
    >
      <div class="p-4 flex justify-between items-center border-b border-gray-200 dark:border-white/5">
        <h1 class="text-lg font-semibold flex items-center gap-2.5 text-gray-900 dark:text-slate-100">
          <div class="w-7 h-7 rounded-lg bg-gradient-to-br from-blue-500 to-violet-500 flex items-center justify-center">
            <MessageSquare class="w-4 h-4 text-white" />
          </div>
          Chats
        </h1>
        <div class="flex items-center gap-1">
          <Button variant="ghost" size="icon" class="text-gray-500 dark:text-slate-400 hover:text-gray-800 dark:hover:text-slate-200" @click="toggle">
            <Sun v-if="theme === 'dark'" class="w-5 h-5" />
            <Moon v-else class="w-5 h-5" />
          </Button>
          <Button variant="ghost" size="icon" class="md:hidden text-gray-500 dark:text-slate-400" @click="toggleSidebar">
            <X class="w-5 h-5"/>
          </Button>
        </div>
      </div>
      
      <ScrollArea class="flex-1">
        <div class="flex flex-col gap-0.5 p-2">
          <div 
            v-for="chat in chatStore.chats" 
            :key="chat.id"
            class="group flex items-center gap-3 p-3 rounded-xl cursor-pointer text-gray-500 dark:text-slate-400 transition-all duration-200 hover:bg-gray-100 dark:hover:bg-white/5 hover:text-gray-800 dark:hover:text-slate-100 relative"
            :class="{ 'bg-gray-100 dark:bg-slate-800/60 text-gray-900 dark:text-slate-100': chat.id === chatStore.activeChatId }"
            @click="selectChat(chat.id)"
          >
            <!-- Avatar -->
            <div class="w-11 h-11 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center shrink-0 shadow-sm">
              <Bot class="w-5 h-5 text-white" />
            </div>
            
            <!-- Content -->
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
                <span class="text-[10px] text-gray-400 dark:text-slate-500 ml-2 shrink-0">
                  {{ chat.updatedAt ? new Date(chat.updatedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '' }}
                </span>
              </div>
              <p class="text-xs text-gray-500 dark:text-slate-500 truncate mt-0.5">
                {{ chat.messages[chat.messages.length - 1]?.content?.slice(0, 60) || 'No messages yet' }}
              </p>
            </div>

            <!-- Actions -->
            <div class="opacity-0 group-hover:opacity-100 transition-opacity">
              <HeadlessMenu as="div" class="relative">
                <MenuButton as="button" @click.stop class="p-1.5 rounded-md hover:bg-gray-200 dark:hover:bg-white/10 text-gray-400 dark:text-slate-400 hover:text-gray-800 dark:hover:text-slate-200 transition-colors">
                  <MoreVertical class="w-4 h-4" />
                </MenuButton>
                <transition
                  enter-active-class="transition duration-100 ease-out"
                  enter-from-class="transform scale-95 opacity-0"
                  enter-to-class="transform scale-100 opacity-100"
                  leave-active-class="transition duration-75 ease-in"
                  leave-from-class="transform scale-100 opacity-100"
                  leave-to-class="transform scale-95 opacity-0"
                >
                  <MenuItems class="absolute right-0 top-8 z-30 w-32 origin-top-right rounded-lg bg-white dark:bg-slate-800 border border-gray-200 dark:border-white/10 shadow-lg focus:outline-none py-1">
                    <MenuItem v-slot="{ active }">
                      <button
                        @click.stop="startRename(chat.id, chat.name, $event)"
                        :class="[active ? 'bg-gray-100 dark:bg-white/10 text-gray-900 dark:text-slate-100' : 'text-gray-700 dark:text-slate-300', 'group flex w-full items-center px-3 py-2 text-xs']"
                      >
                        <Pencil class="w-3.5 h-3.5 mr-2" />
                        Rename
                      </button>
                    </MenuItem>
                    <MenuItem v-slot="{ active }">
                      <button
                        @click.stop="chatStore.deleteChat(chat.id)"
                        :class="[active ? 'bg-red-500/10 text-red-500 dark:text-red-400' : 'text-red-500 dark:text-red-400', 'group flex w-full items-center px-3 py-2 text-xs']"
                      >
                        <Trash2 class="w-3.5 h-3.5 mr-2" />
                        Delete
                      </button>
                    </MenuItem>
                  </MenuItems>
                </transition>
              </HeadlessMenu>
            </div>
          </div>
        </div>
      </ScrollArea>

      <!-- New Chat Button -->
      <div class="p-3 border-t border-gray-200 dark:border-white/5">
        <Button variant="outline" class="w-full justify-start gap-2 bg-gray-50 dark:bg-white/5 border-gray-200 dark:border-white/10 hover:bg-gray-100 dark:hover:bg-white/10 text-gray-900 dark:text-slate-100" @click="createNewChat">
          <Plus class="w-4 h-4" />
          New Chat
        </Button>
      </div>
    </aside>

    <!-- Overlay for mobile -->
    <div 
      v-if="isSidebarOpen" 
      class="fixed inset-0 bg-black/20 dark:bg-black/50 z-15 md:hidden"
      @click="toggleSidebar"
    ></div>

    <!-- Main Chat Area -->
    <main class="flex-1 flex flex-col min-w-0 relative bg-white dark:bg-slate-800/40 md:pt-0 pt-14">
      <ChatArea
        v-if="chatStore.activeChatId"
        :chat-id="chatStore.activeChatId"
      />
    </main>
  </div>
</template>

<style>
/* Any additional ultra custom overides can live here if needed, but Tailwind primarily powers it now */
</style>
