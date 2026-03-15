<script setup lang="ts">
import {
  ScrollAreaCorner,
  ScrollAreaRoot,
  ScrollAreaScrollbar,
  ScrollAreaThumb,
  ScrollAreaViewport,
  type ScrollAreaRootProps,
} from 'radix-vue'
import { cn } from '@/lib/utils'

const props = defineProps<ScrollAreaRootProps & { class?: any }>()
</script>

<template>
  <ScrollAreaRoot
    :type="type"
    :dir="dir"
    :class="cn('relative overflow-hidden', props.class)"
  >
    <ScrollAreaViewport class="h-full w-full rounded-[inherit]">
      <slot />
    </ScrollAreaViewport>
    <ScrollAreaScrollbar
      orientation="vertical"
      class="flex h-full w-2.5 touch-none select-none border-l border-l-transparent p-[1px] transition-colors"
    >
      <ScrollAreaThumb class="relative flex-1 rounded-full bg-border" />
    </ScrollAreaScrollbar>
    <ScrollAreaScrollbar
      orientation="horizontal"
      class="flex h-2.5 flex-col touch-none select-none border-t border-t-transparent p-[1px] transition-colors"
    >
      <ScrollAreaThumb class="relative flex-1 rounded-full bg-border" />
    </ScrollAreaScrollbar>
    <ScrollAreaCorner />
  </ScrollAreaRoot>
</template>
