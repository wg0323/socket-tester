<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import ProjectsPage from './ProjectsPage.vue';
import SelectBox from './SelectBox.vue';
import Sidebar from './Sidebar.vue';
import {
  asciiToBytes,
  bytesToAscii,
  formatHex,
  hexToBytes,
  parseHexText,
} from './utils/hex';
import { extractFrames } from './utils/logback';
import type { ExtractedFrame } from './utils/logback';
import type {
  EnvConfig,
  HistoryItem,
  MessageItem,
  Mode,
  Project,
  ProjectSummary,
  SidebarAction,
} from './types';

type Direction = 'rx' | 'tx' | 'sys';

interface LogPayload {
  ts: string;
  dir: Direction;
  sessionId: string;
  peer: string | null;
  hex: string;
  len: number;
}

interface LogItem extends LogPayload {
  id: number;
  bytes: number[];
  delta: number;
}

interface SessionInfo {
  id: string;
  kind: string;
  local: string;
  peers: string[];
}

interface NetIf {
  name: string;
  ip: string;
}

const MAX_LOGS = 5000;

const mode = ref<Mode>('tcp-client');
const host = ref('127.0.0.1');
const port = ref('7611');
const localPort = ref('0');
const localIp = ref('');
const multicast = ref('');
const target = ref('');
const sessionId = ref('');
const connectError = ref('');
const busy = ref(false);
const netIfs = ref<NetIf[]>([]);

const modeOptions: Array<{ value: Mode; label: string }> = [
  { value: 'tcp-client', label: 'TCP Client' },
  { value: 'tcp-server', label: 'TCP Server' },
  { value: 'udp', label: 'UDP' },
];

const ipOptions = computed<Array<{ value: string; label: string }>>(() => [
  { value: '', label: '全部网卡' },
  ...netIfs.value.map((item) => ({
    value: item.ip,
    label: `${item.ip} · ${item.name}`,
  })),
]);

const sendText = ref('');
const sendHexMode = ref(true);
const sendError = ref('');
const timerEnabled = ref(false);
const timerMs = ref('12000');
const autoReply = ref(false);
const loopInterval = ref('1000');
const loopRunning = ref(false);
const autoReconnect = ref(false);
const hexView = ref(true);
const logFilter = ref('');
const notice = ref('');
const rxTimeoutSec = ref('0');
const nowTick = ref(Date.now());
const lastRxAt = ref(0);

const logs = ref<LogItem[]>([]);
const logBox = ref<HTMLElement | null>(null);
const followTail = ref(true);

const extracted = ref<ExtractedFrame[]>([]);
const extractedIndex = ref(0);

const txBytes = ref(0);
const rxBytes = ref(0);
const txCount = ref(0);
const rxCount = ref(0);

const view = ref<'projects' | 'workspace'>('projects');
const projects = ref<ProjectSummary[]>([]);
const projectsLoading = ref(true);
const projectError = ref('');
const projectId = ref('');
const project = ref<Project>(blankProject());
const dirty = ref(false);

let timer: ReturnType<typeof setInterval> | null = null;
let tickTimer: ReturnType<typeof setInterval> | null = null;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
let logSeq = 0;
let lastLogTs: number | null = null;
let messageSeq = 0;
let historySeq = 0;
let suppressDirty = false;
let reconnecting = false;
let lastAutoReplyAt = 0;
let unlisten: UnlistenFn | null = null;

const connected = computed(
  () => sessionId.value !== '' && !loopRunning.value,
);

const sendByteCount = computed(() => {
  if (sendHexMode.value) {
    return parseHexText(sendText.value).bytes.length;
  }
  return asciiToBytes(unescapeText(sendText.value)).length;
});

const currentFrameMeta = computed(() => {
  const frame = extracted.value[extractedIndex.value];
  if (!frame) {
    return '';
  }
  const dir = frame.dir === 'tx' ? '发' : '收';
  return `${frame.byteLen} 字节 · ${dir} · ${frame.time}`;
});

const visibleLogs = computed(() => {
  const keyword = logFilter.value.trim().toLowerCase();
  if (!keyword) {
    return logs.value;
  }
  const compact = keyword.replace(/\s/g, '');
  return logs.value.filter((item) => {
    if (item.dir === 'sys') {
      return item.hex.toLowerCase().includes(keyword);
    }
    const hex = item.hex.replace(/\s/g, '').toLowerCase();
    return hex.includes(compact) || item.ts.includes(keyword);
  });
});

const rxIdleSec = computed(() => {
  if (lastRxAt.value === 0) {
    return -1;
  }
  return Math.floor((nowTick.value - lastRxAt.value) / 1000);
});

const rxTimedOut = computed(() => {
  const limit = Number(rxTimeoutSec.value);
  return limit > 0 && rxIdleSec.value >= limit;
});

/** ASCII 模式支持 \r \n \t \\ 写法，便于在一行里发控制字符。 */
function unescapeText(text: string): string {
  return text
    .replace(/\\r/g, '\r')
    .replace(/\\n/g, '\n')
    .replace(/\\t/g, '\t')
    .replace(/\\\\/g, '\\');
}

function tsToMs(ts: string): number {
  const parts = ts.split(':');
  if (parts.length !== 3) {
    return 0;
  }
  const hours = Number(parts[0]);
  const minutes = Number(parts[1]);
  const seconds = Number(parts[2]);
  if (
    !Number.isFinite(hours) ||
    !Number.isFinite(minutes) ||
    !Number.isFinite(seconds)
  ) {
    return 0;
  }
  return hours * 3600000 + minutes * 60000 + seconds * 1000;
}

function blankProject(name = '未命名工程'): Project {
  return {
    version: 1,
    name,
    activeEnv: '默认',
    environments: {
      默认: {
        mode: 'tcp-client',
        host: '127.0.0.1',
        port: '7611',
        localPort: '0',
        localIp: '',
        multicast: '',
      },
    },
    messages: [],
    history: [],
  };
}

function toPort(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === '') {
    return 0;
  }
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < 0 || parsed > 65535) {
    return null;
  }
  return parsed;
}

// ---------------------------------------------------------------- 工程

function applyEnv(name: string): void {
  const env = project.value.environments[name];
  if (!env) {
    return;
  }
  project.value.activeEnv = name;
  mode.value = env.mode;
  host.value = env.host;
  port.value = env.port;
  localPort.value = env.localPort;
  localIp.value = env.localIp;
  multicast.value = env.multicast;
  if (connected.value) {
    notice.value = '已切换环境，需断开后重新打开才生效';
  }
}

function currentEnv(): EnvConfig {
  return {
    mode: mode.value,
    host: host.value,
    port: port.value,
    localPort: localPort.value,
    localIp: localIp.value,
    multicast: multicast.value,
  };
}

function saveEnv(): void {
  project.value.environments[project.value.activeEnv] = currentEnv();
}

function addEnv(name: string): void {
  project.value.environments[name] = currentEnv();
  project.value.activeEnv = name;
}

async function loadProjects(): Promise<void> {
  projectsLoading.value = true;
  try {
    projects.value = await invoke<ProjectSummary[]>('list_projects');
    projectError.value = '';
  } catch (error) {
    projectError.value = String(error);
  } finally {
    projectsLoading.value = false;
  }
}

function parseProject(text: string): Project {
  const parsed = JSON.parse(text) as Project;
  if (!parsed || typeof parsed !== 'object' || !parsed.environments) {
    throw new Error('工程文件格式不正确');
  }
  parsed.messages = Array.isArray(parsed.messages) ? parsed.messages : [];
  parsed.history = Array.isArray(parsed.history) ? parsed.history : [];
  if (!parsed.name) {
    parsed.name = '未命名工程';
  }
  return parsed;
}

function enterWorkspace(parsed: Project, id: string): void {
  suppressDirty = true;
  project.value = parsed;
  projectId.value = id;
  const envName = parsed.environments[parsed.activeEnv]
    ? parsed.activeEnv
    : Object.keys(parsed.environments)[0];
  if (envName) {
    applyEnv(envName);
  }
  connectError.value = '';
  notice.value = '';
  sendError.value = '';
  logs.value = [];
  resetCounters();
  dirty.value = false;
  view.value = 'workspace';
  void nextTick(() => {
    suppressDirty = false;
  });
}

async function openProjectById(id: string): Promise<void> {
  try {
    const text = await invoke<string>('read_project', { id });
    enterWorkspace(parseProject(text), id);
  } catch (error) {
    projectError.value = String(error);
  }
}

async function createProject(name: string): Promise<void> {
  const id = `p${Date.now().toString(36)}`;
  try {
    await invoke('write_project', {
      id,
      content: JSON.stringify(blankProject(name), null, 2),
    });
    await openProjectById(id);
  } catch (error) {
    projectError.value = String(error);
  }
}

async function renameProject(id: string, name: string): Promise<void> {
  try {
    const text = await invoke<string>('read_project', { id });
    const parsed = parseProject(text);
    parsed.name = name;
    await invoke('write_project', { id, content: JSON.stringify(parsed, null, 2) });
    if (projectId.value === id) {
      project.value.name = name;
    }
    await loadProjects();
  } catch (error) {
    projectError.value = String(error);
  }
}

async function deleteProject(id: string): Promise<void> {
  try {
    await invoke('delete_project', { id });
    if (projectId.value === id) {
      projectId.value = '';
      project.value = blankProject();
    }
    await loadProjects();
  } catch (error) {
    projectError.value = String(error);
  }
}

/** 只用于接管以前手工保存的 JSON 工程文件。 */
async function importProject(): Promise<void> {
  const selected = await openDialog({
    multiple: false,
    directory: false,
    filters: [{ name: 'Socket Tester 工程', extensions: ['json'] }],
  });
  if (typeof selected !== 'string') {
    return;
  }
  try {
    const text = await invoke<string>('read_project_file', { path: selected });
    const parsed = parseProject(text);
    const id = `p${Date.now().toString(36)}`;
    await invoke('write_project', { id, content: JSON.stringify(parsed, null, 2) });
    await openProjectById(id);
  } catch (error) {
    projectError.value = String(error);
  }
}

async function saveProjectNow(): Promise<void> {
  if (!projectId.value) {
    return;
  }
  try {
    await invoke('write_project', {
      id: projectId.value,
      content: JSON.stringify(project.value, null, 2),
    });
    dirty.value = false;
    if (view.value === 'workspace') {
      connectError.value = '';
    }
  } catch (error) {
    connectError.value = String(error);
  }
}

async function flushSave(): Promise<void> {
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  if (dirty.value) {
    await saveProjectNow();
  }
}

async function backToProjects(): Promise<void> {
  await flushSave();
  stopTimer();
  loopRunning.value = false;
  autoReconnect.value = false;
  if (sessionId.value) {
    const dead = sessionId.value;
    sessionId.value = '';
    await invoke('disconnect', { sessionId: dead }).catch(() => undefined);
  }
  view.value = 'projects';
  await loadProjects();
}

function markDirty(): void {
  dirty.value = true;
  if (!projectId.value) {
    return;
  }
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
  }
  saveTimer = setTimeout(() => {
    void saveProjectNow();
  }, 800);
}

watch(
  project,
  () => {
    if (!suppressDirty) {
      markDirty();
    }
  },
  { deep: true },
);

// ---------------------------------------------------------------- 报文与历史

function applyMessage(id: string): void {
  const item = project.value.messages.find((msg) => msg.id === id);
  if (!item) {
    return;
  }
  sendText.value = item.payload;
  sendHexMode.value = item.hexMode;
}

async function sendMessage(id: string): Promise<void> {
  applyMessage(id);
  await send();
}

function addMessage(name: string, group: string): void {
  const item: MessageItem = {
    id: `m${Date.now()}-${messageSeq}`,
    group: group || '默认',
    name,
    hexMode: sendHexMode.value,
    payload: sendText.value,
  };
  messageSeq += 1;
  project.value.messages.push(item);
}

function applyHistory(id: string): void {
  const item = project.value.history.find((entry) => entry.id === id);
  if (!item) {
    return;
  }
  sendText.value = item.payload;
  sendHexMode.value = item.hexMode;
}

function pushHistory(): void {
  const entry: HistoryItem = {
    id: `h${Date.now()}-${historySeq}`,
    hexMode: sendHexMode.value,
    payload: sendText.value,
    at: new Date().toLocaleTimeString('zh-CN', { hour12: false }),
  };
  historySeq += 1;
  project.value.history.unshift(entry);
  if (project.value.history.length > 50) {
    project.value.history.splice(50);
  }
}

function onSidebarAction(payload: SidebarAction): void {
  switch (payload.type) {
    case 'switch-env':
      applyEnv(payload.name);
      break;
    case 'save-env':
      saveEnv();
      break;
    case 'add-env':
      addEnv(payload.name);
      break;
    case 'apply-message':
      applyMessage(payload.id);
      break;
    case 'send-message':
      void sendMessage(payload.id);
      break;
    case 'delete-message':
      project.value.messages = project.value.messages.filter(
        (msg) => msg.id !== payload.id,
      );
      break;
    case 'rename-message': {
      const item = project.value.messages.find((msg) => msg.id === payload.id);
      if (item) {
        item.name = payload.name;
      }
      break;
    }
    case 'add-message':
      addMessage(payload.name, payload.group);
      break;
    case 'apply-history':
      applyHistory(payload.id);
      break;
    case 'clear-history':
      project.value.history = [];
      break;
    default:
      break;
  }
}

// ---------------------------------------------------------------- 连接与收发

async function boot(): Promise<void> {
  netIfs.value = await invoke<NetIf[]>('list_local_ips');
  await loadProjects();
  unlisten = await listen<LogPayload[]>('log:batch', (event) => {
    for (const payload of event.payload) {
      let delta = 0;
      if (payload.dir !== 'sys') {
        const ms = tsToMs(payload.ts);
        delta = lastLogTs === null ? 0 : Math.max(0, ms - lastLogTs);
        lastLogTs = ms;
      }
      logs.value.push({
        ...payload,
        id: logSeq,
        peer: mode.value === 'tcp-client' ? null : payload.peer,
        bytes: payload.dir === 'sys' ? [] : hexToBytes(payload.hex),
        delta,
      });
      logSeq += 1;
      if (payload.dir === 'rx') {
        rxBytes.value += payload.len;
        rxCount.value += 1;
        lastRxAt.value = Date.now();
        if (
          autoReply.value &&
          sessionId.value !== '' &&
          sendText.value.trim() !== ''
        ) {
          const now = Date.now();
          if (now - lastAutoReplyAt > 300) {
            lastAutoReplyAt = now;
            void send(false);
          }
        }
      } else if (payload.dir === 'tx') {
        txBytes.value += payload.len;
        txCount.value += 1;
      } else if (payload.dir === 'sys') {
        handleLinkLost(payload);
      }
    }
    const overflow = logs.value.length - MAX_LOGS;
    if (overflow > 0) {
      logs.value.splice(0, overflow);
    }
    if (followTail.value) {
      scrollToBottom();
    }
  });
}

async function open(): Promise<void> {
  connectError.value = '';
  const remotePort = toPort(port.value);
  const bindPort = toPort(localPort.value);

  if (bindPort === null) {
    connectError.value = '本地端口无效（0~65535）';
    return;
  }
  if (mode.value === 'tcp-client') {
    if (!host.value.trim()) {
      connectError.value = '请填写目的地址';
      return;
    }
    if (remotePort === null || remotePort === 0) {
      connectError.value = '请填写目的端口';
      return;
    }
  }

  busy.value = true;
  try {
    const info = await invoke<SessionInfo>('connect', {
      req: {
        kind: mode.value,
        host: host.value.trim() || null,
        port: remotePort,
        localIp: localIp.value || null,
        localPort: bindPort,
        multicast: multicast.value.trim() || null,
      },
    });
    sessionId.value = info.id;
    logs.value = [];
    resetCounters();
    target.value = '';
  } catch (error) {
    connectError.value = String(error);
  } finally {
    busy.value = false;
  }
}

async function close(): Promise<void> {
  stopTimer();
  if (!sessionId.value) {
    return;
  }
  await invoke('disconnect', { sessionId: sessionId.value });
  sessionId.value = '';
}

async function send(record = true): Promise<void> {
  sendError.value = '';
  if (!sessionId.value) {
    sendError.value = '请先打开连接';
    return;
  }

  let bytes: number[];
  if (sendHexMode.value) {
    const parsed = parseHexText(sendText.value);
    if (parsed.error) {
      sendError.value = parsed.error;
      return;
    }
    bytes = parsed.bytes;
  } else {
    bytes = asciiToBytes(unescapeText(sendText.value));
  }

  if (bytes.length === 0) {
    sendError.value = '报文内容为空';
    return;
  }

  try {
    await invoke('send_data', {
      sessionId: sessionId.value,
      hex: formatHex(bytes),
      target: target.value.trim() || null,
    });
    if (record) {
      pushHistory();
    }
  } catch (error) {
    sendError.value = String(error);
  }
}

function stopTimer(): void {
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

function handleLinkLost(payload: LogPayload): void {
  if (mode.value !== 'tcp-client' || sessionId.value === '') {
    return;
  }
  if (payload.sessionId !== sessionId.value) {
    return;
  }
  const lost =
    payload.hex.includes('对端已关闭连接') || payload.hex.includes('接收失败');
  if (!lost) {
    return;
  }
  const deadId = sessionId.value;
  sessionId.value = '';
  void invoke('disconnect', { sessionId: deadId }).catch(() => undefined);
  if (autoReconnect.value && !loopRunning.value) {
    void autoReconnectLoop();
  }
}

async function autoReconnectLoop(): Promise<void> {
  if (reconnecting) {
    return;
  }
  const remotePort = toPort(port.value);
  if (remotePort === null || remotePort === 0) {
    return;
  }
  reconnecting = true;
  while (autoReconnect.value && sessionId.value === '' && mode.value === 'tcp-client') {
    try {
      const info = await invoke<SessionInfo>('connect', {
        req: {
          kind: 'tcp-client',
          host: host.value.trim(),
          port: remotePort,
          localIp: localIp.value || null,
          localPort: toPort(localPort.value) ?? 0,
          multicast: null,
        },
      });
      if (!autoReconnect.value || view.value !== 'workspace') {
        void invoke('disconnect', { sessionId: info.id }).catch(() => undefined);
        break;
      }
      sessionId.value = info.id;
      connectError.value = '';
      notice.value = '掉线后已自动重连';
    } catch (error) {
      connectError.value = `自动重连失败，1 秒后重试：${String(error)}`;
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
  }
  reconnecting = false;
}

/** 反复「连接→发送→断开」，用于验证设备断线重连是否稳定。 */
async function toggleLoop(): Promise<void> {
  if (loopRunning.value) {
    loopRunning.value = false;
    return;
  }
  if (mode.value !== 'tcp-client') {
    sendError.value = '反复连接仅支持 TCP Client 模式';
    return;
  }
  const interval = Number(loopInterval.value);
  if (!Number.isFinite(interval) || interval < 0) {
    sendError.value = '反复连接间隔无效';
    return;
  }
  const remotePort = toPort(port.value);
  if (!host.value.trim() || remotePort === null || remotePort === 0) {
    sendError.value = '请填写目的地址与端口';
    return;
  }

  loopRunning.value = true;
  while (loopRunning.value) {
    try {
      const info = await invoke<SessionInfo>('connect', {
        req: {
          kind: 'tcp-client',
          host: host.value.trim(),
          port: remotePort,
          localIp: localIp.value || null,
          localPort: toPort(localPort.value) ?? 0,
          multicast: null,
        },
      });
      sessionId.value = info.id;
      await send(false);
      await invoke('disconnect', { sessionId: info.id });
    } catch (error) {
      sendError.value = String(error);
    }
    sessionId.value = '';
    if (!loopRunning.value) {
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
}

watch(timerEnabled, (enabled) => {
  stopTimer();
  if (!enabled) {
    return;
  }
  const interval = Number(timerMs.value);
  if (!Number.isFinite(interval) || interval < 20) {
    sendError.value = '定时间隔至少 20ms';
    timerEnabled.value = false;
    return;
  }
  timer = setInterval(() => {
    if (!sessionId.value) {
      stopTimer();
      return;
    }
    void send(false);
  }, interval);
});

function onPaste(event: ClipboardEvent): void {
  const text = event.clipboardData?.getData('text') ?? '';
  const frames = extractFrames(text);
  if (frames.length === 0) {
    return;
  }
  event.preventDefault();
  extracted.value = frames;
  applyFrame(0);
}

function applyFrame(index: number): void {
  const total = extracted.value.length;
  if (total === 0) {
    return;
  }
  const next = ((index % total) + total) % total;
  extractedIndex.value = next;
  const frame = extracted.value[next];
  sendText.value = frame.hex;
  sendHexMode.value = true;
  sendError.value = frame.warning;
}

function stepFrame(delta: number): void {
  applyFrame(extractedIndex.value + delta);
}

function markOf(item: LogItem): string {
  if (item.dir === 'tx') {
    return '→';
  }
  if (item.dir === 'rx') {
    return '←';
  }
  return '·';
}

function renderBody(item: LogItem): string {
  if (item.dir === 'sys') {
    return item.hex;
  }
  return hexView.value ? item.hex : bytesToAscii(item.bytes);
}

function scrollToBottom(): void {
  nextTick(() => {
    const box = logBox.value;
    if (box) {
      box.scrollTop = box.scrollHeight;
    }
  });
}

function onLogScroll(): void {
  const box = logBox.value;
  if (!box) {
    return;
  }
  followTail.value = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
}

async function exportLogs(): Promise<void> {
  if (logs.value.length === 0) {
    connectError.value = '没有可导出的日志';
    return;
  }
  const selected = await saveDialog({
    defaultPath: `socket-log-${Date.now()}.txt`,
    filters: [
      { name: '文本', extensions: ['txt'] },
      { name: 'CSV', extensions: ['csv'] },
    ],
  });
  if (typeof selected !== 'string') {
    return;
  }
  const isCsv = selected.toLowerCase().endsWith('.csv');
  const rows = logs.value.map((item) => {
    if (item.dir === 'sys') {
      return isCsv
        ? `${item.ts},sys,0,"${item.hex.replace(/"/g, '""')}"`
        : `${item.ts}  sys  ${item.hex}`;
    }
    const dir = item.dir === 'rx' ? 'RX' : 'TX';
    return isCsv
      ? `${item.ts},${dir},${item.len},${item.hex}`
      : `${item.ts}  ${dir}  ${String(item.len).padStart(4)}B  ${item.hex}`;
  });
  const content = isCsv
    ? `time,direction,length,hex\n${rows.join('\n')}`
    : rows.join('\n');
  try {
    await invoke('write_text_file', { path: selected, content });
    notice.value = `已导出 ${logs.value.length} 条日志`;
    connectError.value = '';
  } catch (error) {
    connectError.value = String(error);
  }
}

function clearLogs(): void {
  logs.value = [];
  resetCounters();
}

function resetCounters(): void {
  txBytes.value = 0;
  rxBytes.value = 0;
  txCount.value = 0;
  rxCount.value = 0;
}

onMounted(() => {
  tickTimer = setInterval(() => {
    nowTick.value = Date.now();
  }, 1000);
  void boot();
});

onUnmounted(() => {
  loopRunning.value = false;
  stopTimer();
  if (tickTimer !== null) {
    clearInterval(tickTimer);
  }
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
  }
  if (unlisten) {
    unlisten();
    unlisten = null;
  }
});
</script>

<template>
  <div class="app">
    <header class="topbar">
      <div class="brand">
        <span class="dot" :class="{ on: connected }"></span>
        Socket Tester
      </div>
      <template v-if="view === 'workspace'">
        <span class="topbar-sep"></span>
        <button class="mini" @click="backToProjects">‹ 工程列表</button>
        <span class="crumb" :title="project.name">{{ project.name }}</span>
      </template>
      <div class="hint">TCP / UDP 报文收发与重放</div>
    </header>

    <ProjectsPage
      v-if="view === 'projects'"
      :projects="projects"
      :loading="projectsLoading"
      :error="projectError"
      @open="openProjectById"
      @create="createProject"
      @rename="renameProject"
      @delete="deleteProject"
      @import="importProject"
    />

    <div v-else class="body">
      <Sidebar :project="project" @action="onSidebarAction" />

      <div class="main-col">
        <section class="conn">
          <div class="conn-row">
            <div class="field">
              <label>模式</label>
              <SelectBox
                v-model="mode"
                :options="modeOptions"
                :disabled="connected"
                class="mode-select"
              />
            </div>

            <div v-if="mode === 'tcp-client'" class="field grow">
              <label>目的地址</label>
              <div class="pair">
                <input v-model="host" :disabled="connected" placeholder="IP 或域名" />
                <span class="sep">:</span>
                <input v-model="port" class="port" :disabled="connected" />
              </div>
            </div>

            <div class="field">
              <label>{{ mode === 'tcp-server' ? '监听端口' : '本地端口' }}</label>
              <input
                v-model="localPort"
                class="port"
                :disabled="connected"
                placeholder="0=任意"
              />
            </div>

            <div class="field">
              <label>本地 IP</label>
              <SelectBox
                v-model="localIp"
                :options="ipOptions"
                :disabled="connected"
                class="ip-select"
              />
            </div>

            <div v-if="mode === 'udp'" class="field">
              <label>组播组</label>
              <input v-model="multicast" :disabled="connected" placeholder="可留空" />
            </div>

            <div class="field actions">
              <button
                v-if="!connected"
                class="primary"
                :disabled="busy || loopRunning"
                @click="open"
              >
                {{ busy ? '连接中…' : '打开' }}
              </button>
              <button v-else class="outline danger" @click="close">断开</button>
            </div>
          </div>

          <div v-if="mode === 'tcp-client'" class="conn-row conn-opts">
            <label class="check">
              <input v-model="autoReconnect" type="checkbox" />
              掉线自动重连
            </label>
            <span class="opt-sep"></span>
            <span class="opt-label">反复连接</span>
            <input
              v-model="loopInterval"
              class="interval"
              :disabled="loopRunning"
              placeholder="间隔"
            />
            <span class="unit">ms</span>
            <button
              class="mini outline"
              :class="{ danger: loopRunning }"
              :disabled="connected"
              @click="toggleLoop"
            >
              {{ loopRunning ? '停止' : '开始' }}
            </button>
          </div>
        </section>

        <div v-if="connectError" class="banner error">{{ connectError }}</div>
        <div v-if="notice" class="banner info">{{ notice }}</div>

        <main class="workspace">
          <section class="panel send-panel">
            <div class="panel-head">
              <span>发送</span>
              <div class="head-tools">
                <label class="check">
                  <input v-model="autoReply" type="checkbox" />
                  收到即发
                </label>
                <label class="check">
                  <input v-model="sendHexMode" type="checkbox" />
                  HEX
                </label>
                <button class="mini" @click="sendText = ''">清空</button>
              </div>
            </div>

            <div v-if="extracted.length > 0" class="frame-nav">
              <span class="ok">日志提取 {{ extracted.length }} 条</span>
              <button class="icon" @click="stepFrame(-1)">◀</button>
              <span class="counter">
                {{ extractedIndex + 1 }}/{{ extracted.length }}
              </span>
              <button class="icon" @click="stepFrame(1)">▶</button>
              <span class="meta">{{ currentFrameMeta }}</span>
              <button class="icon" @click="extracted = []">✕</button>
            </div>

            <textarea
              v-model="sendText"
              class="editor"
              spellcheck="false"
              placeholder="HEX：7E 01 00 0A&#10;直接粘贴 logback 日志片段会自动提取报文"
              @paste="onPaste"
              @keydown.ctrl.enter.prevent="send()"
            ></textarea>

            <div class="send-foot">
              <span class="bytes">{{ sendByteCount }} B</span>
              <input
                v-if="mode !== 'tcp-client'"
                v-model="target"
                class="target"
                :placeholder="
                  mode === 'tcp-server'
                    ? '目标留空=广播全部'
                    : '目标留空=最近对端 ip:port'
                "
              />
              <label class="check">
                <input v-model="timerEnabled" type="checkbox" />
                定时
              </label>
              <input v-model="timerMs" class="interval" :disabled="!timerEnabled" />
              <span class="unit">ms</span>
              <button class="primary send-btn" :disabled="!connected" @click="send()">
                发送
              </button>
            </div>

            <div v-if="sendError" class="banner error">{{ sendError }}</div>
          </section>

          <section class="panel recv-panel">
            <div class="panel-head">
              <span>接收</span>
              <div class="head-tools">
                <input v-model="logFilter" class="filter" placeholder="过滤 hex / 时间" />
                <label class="check">
                  <input v-model="hexView" type="checkbox" />
                  HEX
                </label>
                <button class="mini" @click="exportLogs">导出</button>
                <button class="mini" @click="clearLogs">清空</button>
              </div>
            </div>

            <div ref="logBox" class="logbox" @scroll="onLogScroll">
              <div
                v-for="item in visibleLogs"
                :key="item.id"
                class="log-line"
                :class="item.dir"
              >
                <span class="ts">{{ item.ts }}</span>
                <span class="mark">{{ markOf(item) }}</span>
                <span v-if="item.dir !== 'sys'" class="len">{{ item.len }}B</span>
                <span v-if="item.delta > 0" class="delta">+{{ item.delta }}ms</span>
                <span class="body">{{ renderBody(item) }}</span>
                <span v-if="item.peer" class="peer" :title="item.peer">
                  {{ item.peer }}
                </span>
              </div>
              <div v-if="logs.length === 0" class="empty">
                暂无数据，打开连接后开始收发
              </div>
            </div>
          </section>
        </main>

        <footer class="statusbar">
          <span class="stat">TXD <b>{{ txBytes }}</b> B / {{ txCount }} 帧</span>
          <span class="stat">RXD <b>{{ rxBytes }}</b> B / {{ rxCount }} 帧</span>
          <span class="stat" :class="{ alarm: rxTimedOut }">
            {{ rxIdleSec === -1 ? '尚未收到数据' : '最后接收 ' + rxIdleSec + 's 前' }}{{ rxTimedOut ? ' · 超时' : '' }}
          </span>
          <label class="check">
            超时
            <input v-model="rxTimeoutSec" class="interval tiny" />
            秒
          </label>
          <span class="grow"></span>
          <span class="stat">{{ project.name }}{{ dirty ? ' · 保存中…' : '' }}</span>
          <button class="mini" @click="resetCounters">重新计数</button>
        </footer>
      </div>
    </div>
  </div>
</template>
