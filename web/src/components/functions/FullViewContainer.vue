<template>
  <div
    class="tw-py-[2px]"
    :class="store.state.theme === 'dark' ? 'tw-bg-gray-500' : 'tw-bg-gray-200 '"
  >
    <div class="tw-flex tw-justify-between">
      <div class="tw-flex tw-items-center">
        <q-icon
          v-if="showExpandIcon"
          name="keyboard_arrow_up"
          @click.stop="expanded = !expanded"
          class="tw-mr-1 tw-cursor-pointer tw-transition-all"
          :class="[
            store.state.theme === 'dark'
              ? 'tw-text-gray-100'
              : 'tw-text-gray-500',
            expanded ? 'tw-transform tw-rotate-180' : '',
          ]"
          size="20px"
        />
        <div
          @click="showExpandIcon ? expanded = !expanded : null"
          class="tw-text-[14px] tw-font-bold"
          :class="[
            store.state.theme === 'dark'
              ? 'tw-text-gray-100'
              : 'tw-text-gray-500',
            labelClass,
          ]"
        >
          {{ label }}
        </div>
        <slot name="left" />
      </div>
      <div>
        <slot name="right" />
      </div>
    </div>
    <slot v-if="expanded" />
  </div>
</template>
<script setup lang="ts">
import { defineProps, ref, onMounted, computed, watch, defineEmits } from "vue";
import { useStore } from "vuex";

const props = defineProps({
  name: {
    type: String,
    required: true,
  },
  label: {
    type: String,
    required: true,
  },
  isExpandable: {
    type: Boolean,
    default: true,
  },
  isExpanded: {
    type: Boolean,
    default: false,
  },
  labelClass: {
    type: String,
    default: "",
  },
  showExpandIcon: {
    type: Boolean,
    default: true,
    required: false,
  },
});

const emits = defineEmits(["update:isExpanded"]);

const store = useStore();

const expanded = computed({
  get: () => props.isExpanded,
  set: (value) => emits("update:isExpanded", value),
});
</script>

<style scoped lang="scss"></style>
