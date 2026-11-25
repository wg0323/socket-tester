<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import type { ProjectSummary } from './types';

defineProps<{
  projects: ProjectSummary[];
  loading: boolean;
  error: string;
}>();

const emit = defineEmits<{
  open: [id: string];
  create: [name: string];
  rename: [id: string, name: string];
  delete: [id: string];
  import: [];
}>();

type DialogKind = 'create' | 'rename' | 'delete';

const dialog = ref<DialogKind | null>(null);
const target = ref<ProjectSummary | null>(null);
const nameInput = ref('');
const inputBox = ref<HTMLInputElement | null>(null);
const modalBox = ref<HTMLElement | null>(null);

const title = computed(() => {
  if (dialog.value === 'create') {
    return '新建工程';
  }
  if (dialog.value === 'rename') {
    return '重命名工程';
  }
  return '删除工程';
});

const canSubmit = computed(
  () => dialog.value === 'delete' || nameInput.value.trim() !== '',
);

function startCreate(): void {
  target.value = null;
  nameInput.value = '';
  dialog.value = 'create';
}

function startRename(item: ProjectSummary): void {
  target.value = item;
  nameInput.value = item.name;
  dialog.value = 'rename';
}

function startDelete(item: ProjectSummary): void {
  target.value = item;
  dialog.value = 'delete';
}

function close(): void {
  dialog.value = null;
  target.value = null;
}

function submit(): void {
  const name = nameInput.value.trim();
  if (dialog.value === 'create' && name) {
    emit('create', name);
  } else if (dialog.value === 'rename' && name && target.value) {
    emit('rename', target.value.id, name);
  } else if (dialog.value === 'delete' && target.value) {
    emit('delete', target.value.id);
  } else {
    return;
  }
  close();
}

function formatTime(ms: number): string {
  if (!ms) {
    return '';
  }
  const date = new Date(ms);
  const pad = (value: number): string => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

function initial(name: string): string {
  return name.trim().slice(0, 1).toUpperCase() || 'P';
}

watch(dialog, async (value) => {
  if (!value) {
    return;
  }
  await nextTick();
  if (inputBox.value) {
    inputBox.value.focus();
    inputBox.value.select();
  } else {
    modalBox.value?.focus();
  }
});
</script>

<template>
  <main class="projects-page">
    <div class="projects-head">
      <div>
        <h1>工程</h1>
        <p class="projects-sub">
          {{ loading ? '加载中…' : `共 ${projects.length} 个工程` }}
        </p>
      </div>
      <div class="projects-actions">
        <button class="outline" @click="emit('import')">导入旧工程</button>
        <button class="primary" @click="startCreate">新建工程</button>
      </div>
    </div>

    <div v-if="error" class="banner error">{{ error }}</div>

    <div v-if="projects.length > 0" class="project-grid">
      <div
        v-for="item in projects"
        :key="item.id"
        class="project-card"
        role="button"
        tabindex="0"
        @click="emit('open', item.id)"
        @keydown.enter="emit('open', item.id)"
      >
        <div class="card-top">
          <div class="card-avatar">{{ initial(item.name) }}</div>
          <div class="card-text">
            <div class="card-name" :title="item.name">{{ item.name }}</div>
            <div class="card-meta">
              {{ item.envCount }} 个环境 · {{ item.messageCount }} 条报文
            </div>
          </div>
        </div>
        <div class="card-foot">
          <span class="card-time">{{ formatTime(item.updatedAt) }}</span>
          <div class="card-tools" @click.stop @keydown.enter.stop>
            <button class="link" @click="startRename(item)">重命名</button>
            <button class="link danger" @click="startDelete(item)">删除</button>
          </div>
        </div>
      </div>
    </div>

    <div v-else-if="!loading" class="projects-empty">
      <div class="empty-mark">◫</div>
      <p>还没有工程</p>
      <span>工程用来保存环境、报文和发送历史</span>
      <button class="primary" @click="startCreate">新建工程</button>
    </div>

    <div v-if="dialog" class="modal-mask" @click.self="close">
      <div
        ref="modalBox"
        class="modal"
        tabindex="-1"
        @keydown.esc="close"
      >
        <h2>{{ title }}</h2>
        <p v-if="dialog === 'delete'" class="modal-text">
          删除后无法恢复，确定删除「{{ target?.name }}」？
        </p>
        <input
          v-else
          ref="inputBox"
          v-model="nameInput"
          maxlength="40"
          placeholder="工程名称"
          @keydown.enter="submit"
        />
        <div class="modal-actions">
          <button @click="close">取消</button>
          <button
            class="primary"
            :class="{ danger: dialog === 'delete' }"
            :disabled="!canSubmit"
            @click="submit"
          >
            {{ dialog === 'delete' ? '删除' : '确定' }}
          </button>
        </div>
      </div>
    </div>
  </main>
</template>
