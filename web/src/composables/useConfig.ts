import { ref, computed } from 'vue';
import type { ModelProfile, ProviderSummary } from '@/types';

const API_BASE = import.meta.env.VITE_API_URL || 'http://localhost:3000';

async function apiFetch<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    headers: { 'Content-Type': 'application/json' },
    ...options,
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || `API error: ${res.status}`);
  }
  return res.json();
}

// ── Module-level singleton state ─────────────────────────────
// All components share the same reactive refs (same pattern as useTheme.ts).

const _models = ref<Record<string, ModelProfile>>({});
const _providers = ref<ProviderSummary[]>([]);
const _currentModel = ref<string>('');
const _loading = ref(false);
let _initialized = false;

async function fetchModels() {
  const data = await apiFetch<Record<string, ModelProfile>>('/api/config/models');
  _models.value = data;
}

async function fetchProviders() {
  const data = await apiFetch<ProviderSummary[]>('/api/config/providers');
  _providers.value = data;
}

async function fetchCurrentModel() {
  const data = await apiFetch<{ model: string }>('/api/model/current');
  _currentModel.value = data.model;
}

async function initConfig() {
  if (_loading.value) return;
  _loading.value = true;
  try {
    await Promise.all([fetchModels(), fetchProviders(), fetchCurrentModel()]);
    _initialized = true;
  } finally {
    _loading.value = false;
  }
}

async function createModel(name: string, profile: ModelProfile) {
  await apiFetch('/api/config/models', {
    method: 'POST',
    body: JSON.stringify({ name, ...profile }),
  });
  await fetchModels();
}

async function updateModel(name: string, profile: ModelProfile) {
  await apiFetch(`/api/config/models/${encodeURIComponent(name)}`, {
    method: 'PUT',
    body: JSON.stringify(profile),
  });
  await fetchModels();
}

async function deleteModel(name: string) {
  await apiFetch(`/api/config/models/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
  await fetchModels();
}

async function updateProvider(
  name: string,
  update: Partial<ProviderSummary & { api_key?: string }>,
) {
  await apiFetch(`/api/config/providers/${encodeURIComponent(name)}`, {
    method: 'PUT',
    body: JSON.stringify(update),
  });
  await fetchProviders();
}

async function switchModel(modelId: string) {
  const data = await apiFetch<{ previous: string; current: string }>('/api/model/switch', {
    method: 'POST',
    body: JSON.stringify({ model_id: modelId }),
  });
  _currentModel.value = data.current;
  return data;
}

function setCurrentModel(model: string) {
  _currentModel.value = model;
}

export function useConfig() {
  return {
    models: computed(() => _models.value),
    providers: computed(() => _providers.value),
    currentModel: computed(() => _currentModel.value),
    loading: computed(() => _loading.value),
    initialized: _initialized,
    initConfig,
    fetchModels,
    fetchProviders,
    fetchCurrentModel,
    createModel,
    updateModel,
    deleteModel,
    updateProvider,
    switchModel,
    setCurrentModel,
  };
}
