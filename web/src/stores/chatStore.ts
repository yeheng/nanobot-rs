import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { Chat, Message, MessageStatus } from '@/types';

const STORAGE_KEY = 'gasket_chats';
const LEGACY_KEY = 'gasket_sessions';

/** Migrate legacy steps format to simple thinking/toolCalls/content fields */
const migrateLegacyMessage = (msg: any): Message => {
  if (!msg.steps || msg.steps.length === 0) {
    return msg as Message;
  }

  let thinking = msg.thinking || '';
  let toolCalls = msg.toolCalls || [];
  let content = msg.content || '';

  for (const step of msg.steps) {
    if (step.type === 'thinking' && step.content) {
      thinking += step.content;
    } else if (step.type === 'tool_group' && step.tools) {
      toolCalls = [...toolCalls, ...step.tools];
    } else if (step.type === 'content' && step.content) {
      content += step.content;
    }
  }

  const { steps, ...rest } = msg;
  return {
    ...rest,
    thinking: thinking || undefined,
    toolCalls: toolCalls.length > 0 ? toolCalls : undefined,
    content: content
  } as Message;
};

const loadChats = (): Chat[] => {
  // Try new key first
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved) {
    try {
      return JSON.parse(saved);
    } catch (e) {
      console.error('Failed to parse chats from local storage:', e);
    }
  }

  // Migrate from legacy sessions
  const legacy = localStorage.getItem(LEGACY_KEY);
  if (legacy) {
    try {
      const loadedSessions = JSON.parse(legacy);
      const migrated = loadedSessions.map((session: any) => ({
        ...session,
        messages: session.messages.map(migrateLegacyMessage)
      }));
      localStorage.setItem(STORAGE_KEY, JSON.stringify(migrated));
      return migrated;
    } catch (e) {
      console.error('Failed to migrate legacy sessions:', e);
    }
  }

  return [];
};

export const useChatStore = defineStore('chat', () => {
  const chats = ref<Chat[]>(loadChats());
  const activeChatId = ref<string>('');

  const activeChat = computed(() => chats.value.find(c => c.id === activeChatId.value));
  const activeMessages = computed(() => activeChat.value?.messages || []);

  const createChat = () => {
    const newChat: Chat = {
      id: 'chat_' + Date.now() + '_' + Math.random().toString(36).substr(2, 9),
      name: 'New Chat',
      messages: [
        { id: Date.now().toString(), role: 'system', content: 'Connected to gasket Gateway', timestamp: Date.now() }
      ],
      updatedAt: Date.now()
    };
    chats.value.unshift(newChat);
    activeChatId.value = newChat.id;
    return newChat.id;
  };

  const deleteChat = (id: string) => {
    chats.value = chats.value.filter(c => c.id !== id);
    if (activeChatId.value === id) {
      activeChatId.value = chats.value.length > 0 ? chats.value[0].id : '';
    }
    if (chats.value.length === 0) {
      createChat();
    }
  };

  const setActiveChat = (id: string) => {
    activeChatId.value = id;
  };

  const renameChat = (id: string, name: string) => {
    const chat = chats.value.find(c => c.id === id);
    if (chat) {
      chat.name = name.trim();
    }
  };

  const getOrCreateBotMessage = (chatId: string): Message | null => {
    const chat = chats.value.find(c => c.id === chatId);
    if (!chat) return null;

    const lastMsg = chat.messages[chat.messages.length - 1];
    if (lastMsg && lastMsg.role === 'bot') {
      return lastMsg;
    }

    const newBotMsg: Message = {
      id: Date.now().toString(),
      role: 'bot',
      content: '',
      timestamp: Date.now()
    };
    chat.messages.push(newBotMsg);
    chat.updatedAt = Date.now();
    return newBotMsg;
  };

  const appendMessage = (chatId: string, message: Message) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (chat) {
      chat.messages.push(message);
      chat.updatedAt = Date.now();
      if (chat.name === 'New Chat' && message.role === 'user' && message.content) {
        const sanitizedName = message.content
          .replace(/[\u0000-\u001F\u007F]/g, '')
          .replace(/\s+/g, ' ')
          .trim()
          .slice(0, 50);
        chat.name = sanitizedName + (message.content.length > 50 ? '...' : '');
      }
    }
  };

  const updateMessageStatus = (chatId: string, messageId: string, status: MessageStatus) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (!chat) return;
    const msg = chat.messages.find(m => m.id === messageId);
    if (msg) {
      msg.status = status;
      chat.updatedAt = Date.now();
    }
  };

  const appendToMessage = (chatId: string, messageId: string, content: string, field: 'content' | 'thinking' = 'content') => {
    const chat = chats.value.find(c => c.id === chatId);
    if (!chat) return;
    const msg = chat.messages.find(m => m.id === messageId);
    if (msg) {
      if (field === 'thinking') {
        msg.thinking = (msg.thinking || '') + content;
      } else {
        msg.content = (msg.content || '') + content;
      }
      chat.updatedAt = Date.now();
    }
  };

  const updateMessage = (chatId: string, messageId: string, updates: Partial<Message>) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (!chat) return;
    const msg = chat.messages.find(m => m.id === messageId);
    if (msg) {
      Object.assign(msg, updates);
      chat.updatedAt = Date.now();
    }
  };

  const ensureToolCalls = (chatId: string, messageId: string) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (!chat) return;
    const msg = chat.messages.find(m => m.id === messageId);
    if (msg && !msg.toolCalls) {
      msg.toolCalls = [];
    }
  };

  const pushToolCall = (chatId: string, messageId: string, toolCall: any) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (!chat) return;
    const msg = chat.messages.find(m => m.id === messageId);
    if (msg) {
      if (!msg.toolCalls) msg.toolCalls = [];
      msg.toolCalls.push(toolCall);
      chat.updatedAt = Date.now();
    }
  };

  const updateToolCall = (chatId: string, messageId: string, toolId: string, updates: any) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (!chat) return;
    const msg = chat.messages.find(m => m.id === messageId);
    if (msg && msg.toolCalls) {
      const tool = msg.toolCalls.find(t => t.id === toolId);
      if (tool) {
        Object.assign(tool, updates);
        chat.updatedAt = Date.now();
      }
    }
  };

  const clearChatMessages = (chatId: string) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (chat) {
      chat.messages = [];
      chat.updatedAt = Date.now();
    }
  };

  const setContextStats = (chatId: string, stats: any) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (chat) {
      chat.contextStats = stats;
    }
  };

  const setWatermarkInfo = (chatId: string, info: any) => {
    const chat = chats.value.find(c => c.id === chatId);
    if (chat) {
      chat.watermarkInfo = info;
    }
  };

  // Initialize
  if (chats.value.length === 0) {
    createChat();
  } else if (!activeChatId.value) {
    activeChatId.value = chats.value[0].id;
  }

  return {
    chats,
    activeChatId,
    activeChat,
    activeMessages,
    createChat,
    deleteChat,
    setActiveChat,
    renameChat,
    getOrCreateBotMessage,
    appendMessage,
    updateMessageStatus,
    appendToMessage,
    updateMessage,
    ensureToolCalls,
    pushToolCall,
    updateToolCall,
    clearChatMessages,
    setContextStats,
    setWatermarkInfo
  };
});
