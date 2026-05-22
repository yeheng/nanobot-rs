<script setup lang="ts">
import { ref, reactive } from 'vue';
import { useConfig } from '@/composables/useConfig';
import type { ModelProfile, ProviderSummary } from '@/types';

const emit = defineEmits<{ close: [] }>();

const { models, providers, loading, createModel, updateModel, deleteModel, updateProvider } = useConfig();

const activeTab = ref<'models' | 'providers'>('models');

// ── Models tab state ──
const editingModel = ref<string | null>(null);
const newModelName = ref('');
const newModelProvider = ref('');
const newModelModel = ref('');

async function addModel() {
  if (!newModelName.value || !newModelProvider.value || !newModelModel.value) return;
  await createModel(newModelName.value, {
    provider: newModelProvider.value,
    model: newModelModel.value,
  });
  newModelName.value = '';
  newModelProvider.value = '';
  newModelModel.value = '';
}

async function removeModel(name: string) {
  await deleteModel(name);
}

function startEditModel(name: string) {
  editingModel.value = name;
}

async function saveModelEdit(name: string) {
  const profile = models.value[name];
  if (profile) {
    await updateModel(name, { ...profile });
  }
  editingModel.value = null;
}

// ── Providers tab state ──

interface ProviderEditState {
  api_base: string;
  api_key: string;
  default_model: string;
  proxy_url: string;
  proxy_username: string;
  proxy_password: string;
  client_id: string;
  default_currency: string;
  supports_thinking: boolean;
  extra_headers: { key: string; value: string }[];
}

const editingProvider = ref<string | null>(null);
const providerEdits = reactive<Record<string, ProviderEditState>>({});

function toEditState(p: ProviderSummary): ProviderEditState {
  return {
    api_base: p.api_base,
    api_key: '',
    default_model: p.default_model,
    proxy_url: p.proxy_url || '',
    proxy_username: p.proxy_username || '',
    proxy_password: '',
    client_id: p.client_id || '',
    default_currency: p.default_currency || '',
    supports_thinking: p.supports_thinking,
    extra_headers: Object.entries(p.extra_headers || {}).map(([key, value]) => ({ key, value })),
  };
}

function startEditProvider(name: string) {
  const p = providers.value.find(p => p.name === name);
  if (p) {
    providerEdits[name] = toEditState(p);
    editingProvider.value = name;
  }
}

function addHeader(name: string) {
  providerEdits[name]?.extra_headers.push({ key: '', value: '' });
}

function removeHeader(name: string, idx: number) {
  providerEdits[name]?.extra_headers.splice(idx, 1);
}

async function saveProviderEdit(name: string) {
  const edits = providerEdits[name];
  if (!edits) return;

  const headers: Record<string, string> = {};
  for (const h of edits.extra_headers) {
    if (h.key.trim()) headers[h.key.trim()] = h.value;
  }

  const update: Record<string, any> = {};
  update.api_base = edits.api_base;
  update.api_key = edits.api_key;
  update.default_model = edits.default_model;
  update.proxy_url = edits.proxy_url;
  update.proxy_username = edits.proxy_username;
  update.proxy_password = edits.proxy_password;
  update.client_id = edits.client_id;
  update.default_currency = edits.default_currency;
  update.supports_thinking = edits.supports_thinking;
  update.extra_headers = headers;

  await updateProvider(name, update);
  editingProvider.value = null;
}

</script>

<template>
  <div class="fixed inset-0 z-50 flex justify-end">
    <!-- Backdrop -->
    <div class="absolute inset-0 bg-black/30" @click="emit('close')" />

    <!-- Panel -->
    <div class="relative w-[480px] max-w-full bg-background border-l shadow-xl flex flex-col">
      <!-- Header -->
      <div class="flex items-center justify-between px-4 py-3 border-b">
        <h2 class="text-sm font-semibold">Settings</h2>
        <button @click="emit('close')" class="p-1 rounded hover:bg-secondary">
          <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </div>

      <!-- Tabs -->
      <div class="flex border-b">
        <button
          @click="activeTab = 'models'"
          class="flex-1 px-4 py-2 text-xs font-medium transition-colors"
          :class="activeTab === 'models' ? 'border-b-2 border-primary' : 'text-muted-foreground'"
        >Models</button>
        <button
          @click="activeTab = 'providers'"
          class="flex-1 px-4 py-2 text-xs font-medium transition-colors"
          :class="activeTab === 'providers' ? 'border-b-2 border-primary' : 'text-muted-foreground'"
        >Providers</button>
      </div>

      <!-- Content -->
      <div class="flex-1 overflow-y-auto p-4">
        <div v-if="loading" class="text-center text-xs text-muted-foreground py-8">Loading...</div>

        <!-- Models Tab -->
        <template v-else-if="activeTab === 'models'">
          <!-- Add new model -->
          <div class="mb-4 p-3 border rounded-lg">
            <div class="text-xs font-medium mb-2">Add Model Profile</div>
            <div class="grid grid-cols-3 gap-2">
              <input v-model="newModelName" placeholder="Name" class="cfg-input" />
              <input v-model="newModelProvider" placeholder="Provider" class="cfg-input" />
              <input v-model="newModelModel" placeholder="Model" class="cfg-input" />
            </div>
            <button
              @click="addModel"
              :disabled="!newModelName || !newModelProvider || !newModelModel"
              class="mt-2 px-3 py-1 text-xs rounded bg-primary text-primary-foreground disabled:opacity-50"
            >Add</button>
          </div>

          <!-- Existing models -->
          <div v-for="(profile, name) in models" :key="name" class="p-3 border rounded-lg mb-2">
            <div class="flex items-center justify-between mb-1">
              <span class="text-xs font-medium">{{ name }}</span>
              <div class="flex gap-1">
                <button v-if="editingModel !== name" @click="startEditModel(name as string)"
                  class="text-xs px-2 py-0.5 rounded hover:bg-secondary">Edit</button>
                <button @click="removeModel(name as string)"
                  class="text-xs px-2 py-0.5 rounded hover:bg-destructive/10 text-destructive">Delete</button>
              </div>
            </div>

            <!-- View mode -->
            <div v-if="editingModel !== name" class="text-xs text-muted-foreground">
              {{ profile.provider }}/{{ profile.model }}
              <span v-if="profile.temperature"> · temp={{ profile.temperature }}</span>
            </div>

            <!-- Edit mode -->
            <div v-else class="mt-2 space-y-1">
              <input v-model="models[name].provider" class="cfg-input" placeholder="Provider" />
              <input v-model="models[name].model" class="cfg-input" placeholder="Model" />
              <input v-model.number="models[name].temperature" type="number" step="0.1" class="cfg-input" placeholder="Temperature" />
              <div class="flex gap-2 mt-1">
                <button @click="saveModelEdit(name as string)"
                  class="text-xs px-2 py-0.5 rounded bg-primary text-primary-foreground">Save</button>
                <button @click="editingModel = null"
                  class="text-xs px-2 py-0.5 rounded hover:bg-secondary">Cancel</button>
              </div>
            </div>
          </div>
        </template>

        <!-- Providers Tab -->
        <template v-else-if="activeTab === 'providers'">
          <div v-for="p in providers" :key="p.name" class="p-3 border rounded-lg mb-3">
            <!-- Header -->
            <div class="flex items-center justify-between mb-2">
              <span class="text-xs font-semibold">{{ p.name }}</span>
              <span class="text-[10px] px-1.5 py-0.5 rounded bg-secondary font-mono">{{ p.provider_type }}</span>
            </div>

            <!-- ── View mode ── -->
            <template v-if="editingProvider !== p.name">
              <div class="space-y-0.5 text-[11px] text-muted-foreground">
                <div><span class="text-foreground/50">Base:</span> {{ p.api_base }}</div>
                <div><span class="text-foreground/50">Key:</span> {{ p.api_key_set ? '••••••••' : 'not set' }}</div>
                <div><span class="text-foreground/50">Model:</span> {{ p.default_model }}</div>
                <div v-if="p.proxy_url"><span class="text-foreground/50">Proxy:</span> {{ p.proxy_url }}<span v-if="p.proxy_username"> ({{ p.proxy_username }}:••••)</span></div>
                <div v-if="p.client_id"><span class="text-foreground/50">Client ID:</span> {{ p.client_id }}</div>
                <div v-if="p.default_currency"><span class="text-foreground/50">Currency:</span> {{ p.default_currency }}</div>
                <div><span class="text-foreground/50">Thinking:</span> {{ p.supports_thinking ? 'Yes' : 'No' }}</div>
                <div v-if="p.extra_headers && Object.keys(p.extra_headers).length > 0">
                  <span class="text-foreground/50">Headers:</span> {{ Object.keys(p.extra_headers).join(', ') }}
                </div>
              </div>
              <button @click="startEditProvider(p.name)"
                class="mt-2 text-xs px-2 py-0.5 rounded hover:bg-secondary">Edit</button>
            </template>

            <!-- ── Edit mode ── -->
            <template v-else>
              <div class="mt-1 space-y-2">
                <!-- Connection -->
                <div class="cfg-group">
                  <div class="cfg-label">API Base</div>
                  <input v-model="providerEdits[p.name]!.api_base" class="cfg-input" />
                </div>
                <div class="cfg-group">
                  <div class="cfg-label">API Key <span class="text-muted-foreground font-normal">(leave empty to keep current)</span></div>
                  <input v-model="providerEdits[p.name]!.api_key" type="password" class="cfg-input" placeholder="Enter new key to update" />
                </div>

                <!-- Model -->
                <div class="cfg-group">
                  <div class="cfg-label">Default Model</div>
                  <input v-model="providerEdits[p.name]!.default_model" class="cfg-input" />
                </div>
                <div class="cfg-group flex items-center gap-2">
                  <input type="checkbox" v-model="providerEdits[p.name]!.supports_thinking" class="rounded" />
                  <span class="cfg-label !mb-0">Supports Thinking</span>
                </div>

                <!-- Proxy -->
                <div class="border-t pt-2 mt-1">
                  <div class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1">Proxy</div>
                </div>
                <div class="cfg-group">
                  <div class="cfg-label">Proxy URL</div>
                  <input v-model="providerEdits[p.name]!.proxy_url" class="cfg-input" placeholder="http://127.0.0.1:7890" />
                </div>
                <div class="grid grid-cols-2 gap-2">
                  <div class="cfg-group">
                    <div class="cfg-label">Username</div>
                    <input v-model="providerEdits[p.name]!.proxy_username" class="cfg-input" />
                  </div>
                  <div class="cfg-group">
                    <div class="cfg-label">Password</div>
                    <input v-model="providerEdits[p.name]!.proxy_password" type="password" class="cfg-input" />
                  </div>
                </div>

                <!-- Advanced -->
                <div class="border-t pt-2 mt-1">
                  <div class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1">Advanced</div>
                </div>
                <div class="grid grid-cols-2 gap-2">
                  <div class="cfg-group">
                    <div class="cfg-label">Client ID</div>
                    <input v-model="providerEdits[p.name]!.client_id" class="cfg-input" />
                  </div>
                  <div class="cfg-group">
                    <div class="cfg-label">Currency</div>
                    <input v-model="providerEdits[p.name]!.default_currency" class="cfg-input" placeholder="USD" />
                  </div>
                </div>

                <!-- Extra Headers -->
                <div class="border-t pt-2 mt-1">
                  <div class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1">Extra Headers</div>
                </div>
                <div v-for="(h, idx) in providerEdits[p.name]!.extra_headers" :key="idx" class="flex gap-1 items-center">
                  <input v-model="h.key" class="cfg-input flex-1" placeholder="Header name" />
                  <input v-model="h.value" class="cfg-input flex-1" placeholder="Value" />
                  <button @click="removeHeader(p.name, idx)" class="text-xs px-1.5 py-1 rounded hover:bg-destructive/10 text-destructive shrink-0">&times;</button>
                </div>
                <button @click="addHeader(p.name)" class="text-xs text-primary hover:underline">+ Add Header</button>

                <!-- Actions -->
                <div class="flex gap-2 pt-2 border-t mt-1">
                  <button @click="saveProviderEdit(p.name)"
                    class="text-xs px-3 py-1 rounded bg-primary text-primary-foreground">Save</button>
                  <button @click="editingProvider = null"
                    class="text-xs px-3 py-1 rounded hover:bg-secondary">Cancel</button>
                </div>
              </div>
            </template>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cfg-input {
  @apply w-full px-2 py-1 text-xs border rounded bg-background;
}
.cfg-label {
  @apply text-[11px] font-medium mb-0.5;
}
.cfg-group {
  @apply flex flex-col;
}
</style>
