import { computed, ref, watch } from 'vue'

const STORAGE_KEY = 'gasket_theme_v2'
const LEGACY_KEY = 'gasket_theme'

export type ThemeMode = 'light' | 'dark'
export type ThemeHue = 'zinc' | 'blue' | 'rose' | 'emerald' | 'amber' | 'violet'

export interface ThemeState {
  mode: ThemeMode
  hue: ThemeHue
}

const HUES: ThemeHue[] = ['zinc', 'blue', 'rose', 'emerald', 'amber', 'violet']

function getInitialState(): ThemeState {
  // Try new format first
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored) {
      const parsed = JSON.parse(stored)
      if (parsed.mode && parsed.hue && HUES.includes(parsed.hue)) {
        return { mode: parsed.mode, hue: parsed.hue }
      }
    }
  } catch { /* ignore */ }

  // Migrate from legacy single-value theme
  const legacy = localStorage.getItem(LEGACY_KEY) as ThemeMode | null
  if (legacy === 'light' || legacy === 'dark') {
    return { mode: legacy, hue: 'zinc' }
  }

  // System preference
  const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches
  return { mode: prefersDark ? 'dark' : 'light', hue: 'zinc' }
}

export function useTheme() {
  const state = ref<ThemeState>(getInitialState())

  const apply = (s: ThemeState) => {
    const root = document.documentElement
    if (s.mode === 'light') {
      root.classList.remove('dark')
    } else {
      root.classList.add('dark')
    }
    root.setAttribute('data-hue', s.hue)
    localStorage.setItem(STORAGE_KEY, JSON.stringify(s))
  }

  apply(state.value)

  watch(state, (s) => {
    apply(s)
  }, { deep: true })

  const setMode = (mode: ThemeMode) => {
    state.value.mode = mode
  }

  const setHue = (hue: ThemeHue) => {
    state.value.hue = hue
  }

  const cycleMode = () => {
    state.value.mode = state.value.mode === 'light' ? 'dark' : 'light'
  }

  return {
    mode: computed(() => state.value.mode),
    hue: computed(() => state.value.hue),
    state,
    setMode,
    setHue,
    cycleMode,
    hues: HUES,
  }
}
