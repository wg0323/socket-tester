<script setup lang="ts">
import { computed, ref } from 'vue';
import SelectBox from './SelectBox.vue';
import type { MessageItem, Project, SidebarAction } from './types';

const props = defineProps<{
  project: Project;
}>();

const emit = defineEmits<{
  action: [payload: SidebarAction];
}>();

const tab = ref<'messages' | 'history'>('messages');
const newMessageOpen = ref(false);
const newMessageName = ref('');
const newMessageGroup = ref('');
const newEnvOpen = ref(false);
const newEnvName = ref('');
const editingId = ref('');
const editingName = ref('');

const groups = computed<Array<[string, MessageItem[]]>>(() => {
  const map = new Map<string, MessageItem[]>();
  for (const item of props.project.messages) {
    const key = item.group || '默认';
    const list = map.get(key);
    if (list) {
      list.push(item);
    } else {
      map.set(key, [item]);
    }
  }
  return Array.from(map.entries());
});

const envNames = computed(() => Object.keys(props.project.environments));

const envOptions = computed(() =>
  envNames.value.map((name) => ({ value: name, label: name })),
);

function submitMessage(): void {
  const name = newMessageName.value.trim();
  if (!name) {
    return;
  }
  emit('action', {
    type: 'add-message',
    name,
    group: newMessageGroup.value.trim(),
  });
  newMessageName.value = '';
  newMessageGroup.value = '';
  newMessageOpen.value = false;
}

function submitEnv(): void {
  const name = newEnvName.value.trim();
  if (!name) {
    return;
  }
  emit('action', { type: 'add-env', name });
  newEnvName.value = '';
  newEnvOpen.value = false;
}

function switchEnv(name: string): void {
  emit('action', { type: 'switch-env', name });
}

function startRename(item: MessageItem): void {
  editingId.value = item.id;
  editingName.value = item.name;
}

function finishRename(): void {
  if (editingId.value && editingName.value.trim()) {
    emit('action', {
      type: 'rename-message',
      id: editingId.value,
      name: editingName.value.trim(),
    });
  }
  editingId.value = '';
  editingName.value = '';
}
</script>

<template>
  <aside class="sidebar">
    <div class="side-block">
      <div class="side-head">
        <span>环境</span>
        <button class="icon" @click="newEnvOpen = !newEnvOpen">+</button>
      </div>
      <SelectBox
        :model-value="project.activeEnv"
        :options="envOptions"
        class="env-select"
        @update:model-value="switchEnv"
      />
      <button class="link full" @click="emit('action', { type: 'save-env' })">
        用当前连接参数更新
      </button>
      <div v-if="newEnvOpen" class="inline-form">
        <input
          v-model="newEnvName"
          placeholder="环境名"
          @keydown.enter="submitEnv"
        />
        <button class="mini" @click="submitEnv">确定</button>
      </div>
    </div>

    <div class="side-block grow">
      <div class="side-tabs">
        <button
          :class="{ active: tab === 'messages' }"
          @click="tab = 'messages'"
        >
          报文
        </button>
        <button
          :class="{ active: tab === 'history' }"
          @click="tab = 'history'"
        >
          历史
        </button>
      </div>

      <div v-if="tab === 'messages'" class="side-list">
        <div v-for="[group, items] in groups" :key="group" class="group">
          <div class="group-name">{{ group }}</div>
          <div
            v-for="item in items"
            :key="item.id"
            class="msg-item"
            @click="emit('action', { type: 'apply-message', id: item.id })"
            @dblclick="emit('action', { type: 'send-message', id: item.id })"
          >
            <input
              v-if="editingId === item.id"
              v-model="editingName"
              class="rename"
              @blur="finishRename"
              @keydown.enter="finishRename"
            />
            <span
              v-else
              class="msg-name"
              @dblclick.stop="startRename(item)"
            >
              {{ item.name }}
            </span>
            <button
              v-if="editingId !== item.id"
              class="ghost"
              title="删除"
              @click.stop="emit('action', { type: 'delete-message', id: item.id })"
            >
              ×
            </button>
          </div>
        </div>

        <div v-if="groups.length === 0" class="empty-small">还没有报文</div>

        <div class="add-message">
          <button
            v-if="!newMessageOpen"
            class="link full"
            @click="newMessageOpen = true"
          >
            + 存为报文
          </button>
          <div v-else class="inline-form col">
            <input
              v-model="newMessageName"
              placeholder="报文名称"
              @keydown.enter="submitMessage"
            />
            <input
              v-model="newMessageGroup"
              placeholder="分组（可空）"
              @keydown.enter="submitMessage"
            />
            <div class="row-btns">
              <button class="mini" @click="submitMessage">确定</button>
              <button class="mini" @click="newMessageOpen = false">取消</button>
            </div>
          </div>
        </div>
      </div>

      <div v-else class="side-list">
        <div
          v-for="item in project.history"
          :key="item.id"
          class="msg-item"
          @click="emit('action', { type: 'apply-history', id: item.id })"
        >
          <span class="msg-name mono">{{ item.at }}</span>
          <span class="msg-preview">{{ item.payload.slice(0, 16) }}</span>
        </div>
        <div v-if="project.history.length === 0" class="empty-small">
          还没有发送记录
        </div>
        <button
          v-if="project.history.length > 0"
          class="link full"
          @click="emit('action', { type: 'clear-history' })"
        >
          清空历史
        </button>
      </div>
    </div>
  </aside>
</template>
