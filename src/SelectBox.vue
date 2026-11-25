<script setup lang="ts" generic="T extends string">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';

const props = defineProps<{
  modelValue: T;
  options: ReadonlyArray<{ value: T; label: string }>;
  disabled?: boolean;
  placeholder?: string;
}>();

const emit = defineEmits<{
  'update:modelValue': [value: T];
}>();

const open = ref(false);
const trigger = ref<HTMLButtonElement | null>(null);
const popup = ref<HTMLElement | null>(null);
const activeIndex = ref(-1);
const position = ref({ left: 0, top: 0, minWidth: 0, maxWidth: 260, maxHeight: 300 });

const current = computed(() =>
  props.options.find((item) => item.value === props.modelValue),
);
const label = computed(() => current.value?.label ?? props.placeholder ?? '');

const popupStyle = computed(() => ({
  left: `${position.value.left}px`,
  top: `${position.value.top}px`,
  minWidth: `${position.value.minWidth}px`,
  maxWidth: `${position.value.maxWidth}px`,
  maxHeight: `${position.value.maxHeight}px`,
}));

function updatePosition(): void {
  const el = trigger.value;
  if (!el) {
    return;
  }
  const rect = el.getBoundingClientRect();
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  const left = Math.min(
    Math.max(12, rect.left),
    Math.max(12, viewportWidth - 272),
  );
  const maxWidth = Math.max(180, viewportWidth - left - 12);
  const estimated = Math.min(props.options.length * 34 + 12, 300);
  const belowTop = rect.bottom + 6;
  const aboveTop = rect.top - 6 - estimated;
  const openUp =
    belowTop + estimated > viewportHeight - 12 &&
    aboveTop >= 12 &&
    aboveTop > belowTop;
  const maxHeight = openUp
    ? Math.min(300, rect.top - 18)
    : Math.min(300, viewportHeight - rect.bottom - 18);
  position.value = {
    left,
    top: openUp ? Math.max(12, aboveTop) : belowTop,
    minWidth: Math.min(rect.width, maxWidth),
    maxWidth,
    maxHeight: Math.max(120, maxHeight),
  };
}

function scrollActiveIntoView(): void {
  popup.value
    ?.querySelector<HTMLElement>('.select-option.active')
    ?.scrollIntoView({ block: 'nearest' });
}

function setActive(index: number): void {
  activeIndex.value = index;
  void nextTick(scrollActiveIntoView);
}

function moveActive(step: number): void {
  const total = props.options.length;
  if (total === 0) {
    return;
  }
  const next = activeIndex.value + step;
  setActive(next < 0 ? total - 1 : next >= total ? 0 : next);
}

function openMenu(): void {
  if (props.disabled || props.options.length === 0 || open.value) {
    return;
  }
  updatePosition();
  activeIndex.value = Math.max(
    0,
    props.options.findIndex((item) => item.value === props.modelValue),
  );
  open.value = true;
  document.addEventListener('pointerdown', onOutsidePointer, true);
  window.addEventListener('resize', closeMenu);
  window.addEventListener('blur', closeMenu);
  window.addEventListener('scroll', onWindowScroll, true);
  void nextTick(scrollActiveIntoView);
}

function closeMenu(): void {
  if (!open.value) {
    return;
  }
  open.value = false;
  document.removeEventListener('pointerdown', onOutsidePointer, true);
  window.removeEventListener('resize', closeMenu);
  window.removeEventListener('blur', closeMenu);
  window.removeEventListener('scroll', onWindowScroll, true);
}

function onOutsidePointer(event: PointerEvent): void {
  const target = event.target as Node;
  if (trigger.value?.contains(target) || popup.value?.contains(target)) {
    return;
  }
  closeMenu();
}

function onWindowScroll(event: Event): void {
  if (event.target instanceof Node && popup.value?.contains(event.target)) {
    return;
  }
  closeMenu();
}

function toggle(): void {
  if (open.value) {
    closeMenu();
  } else {
    openMenu();
  }
}

function choose(option: { value: T; label: string } | undefined): void {
  if (!option) {
    return;
  }
  if (option.value !== props.modelValue) {
    emit('update:modelValue', option.value);
  }
  closeMenu();
  trigger.value?.focus();
}

function onTriggerKeydown(event: KeyboardEvent): void {
  if (props.disabled) {
    return;
  }
  if (!open.value) {
    if (
      event.key === 'ArrowDown' ||
      event.key === 'ArrowUp' ||
      event.key === 'Enter' ||
      event.key === ' '
    ) {
      event.preventDefault();
      openMenu();
    }
    return;
  }
  switch (event.key) {
    case 'ArrowDown':
      event.preventDefault();
      moveActive(1);
      break;
    case 'ArrowUp':
      event.preventDefault();
      moveActive(-1);
      break;
    case 'Home':
      event.preventDefault();
      setActive(0);
      break;
    case 'End':
      event.preventDefault();
      setActive(props.options.length - 1);
      break;
    case 'Enter':
    case ' ':
      event.preventDefault();
      choose(props.options[activeIndex.value]);
      break;
    case 'Escape':
      event.preventDefault();
      closeMenu();
      break;
    case 'Tab':
      closeMenu();
      break;
    default:
      break;
  }
}

watch(
  () => props.disabled,
  (disabled) => {
    if (disabled) {
      closeMenu();
    }
  },
);

onBeforeUnmount(closeMenu);
</script>

<template>
  <div class="selectbox" :class="{ open }">
    <button
      ref="trigger"
      type="button"
      class="select-trigger"
      :class="{ open }"
      :disabled="disabled"
      role="combobox"
      aria-haspopup="listbox"
      :aria-expanded="open"
      @click="toggle"
      @keydown="onTriggerKeydown"
    >
      <span class="select-value" :class="{ empty: !current }">{{ label }}</span>
      <span class="select-chevron"></span>
    </button>

    <Teleport to="body">
      <div
        v-if="open"
        ref="popup"
        class="select-popup"
        :style="popupStyle"
        role="listbox"
      >
        <button
          v-for="(option, index) in options"
          :key="option.value"
          type="button"
          class="select-option"
          :class="{
            active: index === activeIndex,
            selected: option.value === modelValue,
          }"
          role="option"
          :aria-selected="option.value === modelValue"
          @mouseenter="setActive(index)"
          @click="choose(option)"
        >
          <span class="select-option-label">{{ option.label }}</span>
          <span
            v-if="option.value === modelValue"
            class="select-option-check"
          ></span>
        </button>
      </div>
    </Teleport>
  </div>
</template>
