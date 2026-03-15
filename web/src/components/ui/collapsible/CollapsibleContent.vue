<script setup lang="ts">
import { CollapsibleContent as RCollapsibleContent, type CollapsibleContentProps } from 'radix-vue'
import { cn } from '@/lib/utils'

const props = defineProps<CollapsibleContentProps & { class?: any }>()
</script>

<template>
  <RCollapsibleContent
    v-bind="props"
    :class="cn('overflow-hidden text-sm data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down', props.class)"
  >
    <slot />
  </RCollapsibleContent>
</template>
