<script setup lang="ts">
import { CollapsibleTrigger as RCollapsibleTrigger, type CollapsibleTriggerProps } from 'radix-vue'

const props = defineProps<CollapsibleTriggerProps>()
</script>

<template>
  <RCollapsibleTrigger v-bind="props">
    <slot />
  </RCollapsibleTrigger>
</template>
