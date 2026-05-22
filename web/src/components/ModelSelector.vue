<script setup lang="ts">
import { ref, computed } from 'vue';
import { onClickOutside } from '@vueuse/core';
import { useConfig } from '@/composables/useConfig';

const { models, currentModel, switchModel } = useConfig();

const open = ref(false);
const switching = ref(false);
const dropdownRef = ref<HTMLElement | null>(null);

onClickOutside(dropdownRef, () => {
  open.value = false;
});

async function selectModel(modelId: string) {
  if (switching.value) return;
  switching.value = true;
  try {
    const profile = models.value[modelId];
    const id = profile ? `${profile.provider}/${profile.model}` : modelId;
    await switchModel(id);
    open.value = false;
  } catch (e) {
    console.error('Failed to switch model:', e);
  } finally {
    switching.value = false;
  }
}

const options = computed(() => {
  const result: { id: string; label: string; provider: string }[] = [];
  for (const [name, profile] of Object.entries(models.value)) {
    result.push({
      id: name,
      label: `${name} (${profile.provider}/${profile.model})`,
      provider: profile.provider,
    });
  }
  return result;
});
</script>

<template>
  <div class="relative" ref="dropdownRef">
    <button
      @click="open = !open"
      class="flex items-center gap-1.5 px-2 py-1 text-xs rounded-md bg-secondary/50 hover:bg-secondary transition-colors"
      :title="'Current model: ' + currentModel"
    >
      <span class="h-2 w-2 rounded-full bg-emerald-500 shrink-0" />
      <span class="truncate max-w-[120px]">{{ currentModel || 'default' }}</span>
      <svg class="h-3 w-3 shrink-0 opacity-50" viewBox="0 0 20 20" fill="currentColor">
        <path
          fill-rule="evenodd"
          d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
          clip-rule="evenodd"
        />
      </svg>
    </button>

    <div
      v-if="open"
      class="absolute right-0 top-full mt-1 z-50 min-w-[200px] max-h-64 overflow-y-auto rounded-lg border bg-popover shadow-lg"
    >
      <button
        v-for="opt in options"
        :key="opt.id"
        @click="selectModel(opt.id)"
        :disabled="switching"
        class="w-full text-left px-3 py-2 text-xs hover:bg-accent transition-colors disabled:opacity-50"
      >
        {{ opt.label }}
      </button>
      <div v-if="options.length === 0" class="px-3 py-2 text-xs text-muted-foreground">
        No model profiles configured
      </div>
    </div>
  </div>
</template>
