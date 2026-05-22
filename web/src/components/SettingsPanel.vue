<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { useConfig } from '@/composables/useConfig';
import type { ModelProfile, ProviderSummary } from '@/types';

const emit = defineEmits<{ close: [] }>();

const { models, providers, loading, fetchModels, fetchProviders, createModel, updateModel, deleteModel, updateProvider } = useConfig();

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
const editingProvider = ref<string | null>(null);
const providerEdits = ref<Record<string, { api_base: string; api_key: string; default_model: string }>>({});

function startEditProvider(name: string) {
  editingProvider.value = name;
  const p = providers.value.find(p => p.name === name);
  if (p) {
    providerEdits.value[name] = { api_base: p.api_base, api_key: '', default_model: p.default_model };
  }
}

async function saveProviderEdit(name: string) {
  const edits = providerEdits.value[name];
  if (edits) {
    const update: Record<string, string> = {};
    if (edits.api_base) update.api_base = edits.api_base;
    if (edits.api_key) update.api_key = edits.api_key;
    if (edits.default_model) update.default_model = edits.default_model;
    await updateProvider(name, update);
  }
  editingProvider.value = null;
}

onMounted(async () => {
  await Promise.all([fetchModels(), fetchProviders()]);
});
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
          <div v-for="p in providers" :key="p.name" class="p-3 border rounded-lg mb-2">
            <div class="flex items-center justify-between mb-1">
              <span class="text-xs font-medium">{{ p.name }}</span>
              <span class="text-xs px-1.5 py-0.5 rounded bg-secondary">{{ p.provider_type }}</span>
            </div>
            <div class="text-xs text-muted-foreground space-y-0.5">
              <div>{{ p.api_base }}</div>
              <div>API Key: {{ p.api_key_set ? '••••••••' : 'not set' }}</div>
              <div>Default: {{ p.default_model }}</div>
            </div>

            <!-- Edit mode -->
            <div v-if="editingProvider === p.name" class="mt-2 space-y-1">
              <input v-model="providerEdits[p.name]!.api_base" class="cfg-input" placeholder="API Base" />
              <input v-model="providerEdits[p.name]!.api_key" type="password" class="cfg-input" placeholder="New API Key" />
              <input v-model="providerEdits[p.name]!.default_model" class="cfg-input" placeholder="Default Model" />
              <div class="flex gap-2 mt-1">
                <button @click="saveProviderEdit(p.name)"
                  class="text-xs px-2 py-0.5 rounded bg-primary text-primary-foreground">Save</button>
                <button @click="editingProvider = null"
                  class="text-xs px-2 py-0.5 rounded hover:bg-secondary">Cancel</button>
              </div>
            </div>
            <button v-else @click="startEditProvider(p.name)"
              class="mt-2 text-xs px-2 py-0.5 rounded hover:bg-secondary">Edit</button>
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
</style>
