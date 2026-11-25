export type Mode = 'tcp-client' | 'tcp-server' | 'udp';

/** 一套连接参数，切环境就是换这套值。 */
export interface EnvConfig {
  mode: Mode;
  host: string;
  port: string;
  localPort: string;
  localIp: string;
  multicast: string;
}

/** 保存的一条发送内容，不含目标，目标由当前环境决定。 */
export interface MessageItem {
  id: string;
  group: string;
  name: string;
  hexMode: boolean;
  payload: string;
}

export interface HistoryItem {
  id: string;
  hexMode: boolean;
  payload: string;
  at: string;
}

export interface Project {
  version: number;
  name: string;
  activeEnv: string;
  environments: Record<string, EnvConfig>;
  messages: MessageItem[];
  history: HistoryItem[];
}

/** 工程列表页用的摘要，不含报文体。 */
export interface ProjectSummary {
  id: string;
  name: string;
  envCount: number;
  messageCount: number;
  updatedAt: number;
}

export type SidebarAction =
  | { type: 'switch-env'; name: string }
  | { type: 'save-env' }
  | { type: 'add-env'; name: string }
  | { type: 'apply-message'; id: string }
  | { type: 'send-message'; id: string }
  | { type: 'delete-message'; id: string }
  | { type: 'rename-message'; id: string; name: string }
  | { type: 'add-message'; name: string; group: string }
  | { type: 'apply-history'; id: string }
  | { type: 'clear-history' };
