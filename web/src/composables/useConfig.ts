import { ref } from 'vue';
import type { ModelProfile, ProviderSummary } from '@/types';

const API_BASE = ''; // Same origin

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

export function useConfig() {
  const models = ref<Record<string, ModelProfile>>({});
  const providers = ref<ProviderSummary[]>([]);
  const currentModel = ref<string>('');
  const loading = ref(false);

  async function fetchModels() {
    loading.value = true;
    try {
      const data = await apiFetch<Record<string, ModelProfile>>('/api/config/models');
      models.value = data;
    } finally {
      loading.value = false;
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

  async function fetchProviders() {
    loading.value = true;
    try {
      const data = await apiFetch<ProviderSummary[]>('/api/config/providers');
      providers.value = data;
    } finally {
      loading.value = false;
    }
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

  async function fetchCurrentModel() {
    const data = await apiFetch<{ model: string }>('/api/model/current');
    currentModel.value = data.model;
  }

  async function switchModel(modelId: string) {
    const data = await apiFetch<{ previous: string; current: string }>('/api/model/switch', {
      method: 'POST',
      body: JSON.stringify({ model_id: modelId }),
    });
    currentModel.value = data.current;
    return data;
  }

  return {
    models,
    providers,
    currentModel,
    loading,
    fetchModels,
    createModel,
    updateModel,
    deleteModel,
    fetchProviders,
    updateProvider,
    fetchCurrentModel,
    switchModel,
  };
}
