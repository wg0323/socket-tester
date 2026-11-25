/**
 * 从 logback 日志里提取报文。
 *
 * 只认日志自己声明的锚点，不做"猜哪段是报文"这种不可靠的启发式：
 *   [发送报文] ... 帧=4040...2323
 *   [收] 完整报文(57字节): 404001...2323
 * 提取结果带着声明长度，用来交叉校验有没有复制漏字符。
 */

import { formatHex, hexToBytes } from './hex';

export interface ExtractedFrame {
  hex: string;
  byteLen: number;
  dir: 'tx' | 'rx';
  time: string;
  declaredLen?: number;
  source: string;
  warning: string;
}

const TS_SPLIT = /\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}[.,]\d{1,3}/g;
const TS_ONE = /(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2}:\d{2}[.,]\d{1,3})/;
const TX_FRAME = /帧\s*=\s*([0-9A-Fa-f]+)/;
const RX_FRAME = /完整报文\s*[（(]\s*(\d+)\s*字节\s*[)）]\s*[:：]\s*([0-9A-Fa-f]+)/;

/** 解析失败的 hex 也要原样展示，方便用户看出哪一位多了或少了。 */
function formatLoose(hex: string): string {
  const pairs = hex.match(/[\s\S]{1,2}/g) ?? [];
  return pairs.map((pair) => pair.toUpperCase()).join(' ');
}

/** 一段日志里可能连着贴了好几条记录，按时间戳把它们切开。 */
function splitRecords(text: string): string[] {
  const marks: number[] = [];
  TS_SPLIT.lastIndex = 0;
  let match = TS_SPLIT.exec(text);
  while (match !== null) {
    marks.push(match.index);
    match = TS_SPLIT.exec(text);
  }
  if (marks.length === 0) {
    return [text];
  }
  const records: string[] = [];
  if (marks[0] > 0) {
    records.push(text.slice(0, marks[0]));
  }
  for (let i = 0; i < marks.length; i += 1) {
    const end = i + 1 < marks.length ? marks[i + 1] : text.length;
    records.push(text.slice(marks[i], end));
  }
  return records;
}

export function extractFrames(text: string): ExtractedFrame[] {
  const frames: ExtractedFrame[] = [];

  for (const record of splitRecords(text)) {
    const timeMatch = record.match(TS_ONE);
    const time = timeMatch ? timeMatch[2].replace(',', '.') : '';

    let hex = '';
    let dir: 'tx' | 'rx' = 'rx';
    let source = '';
    let declaredLen: number | undefined;

    const rx = record.match(RX_FRAME);
    if (rx) {
      declaredLen = Number(rx[1]);
      hex = rx[2];
      dir = 'rx';
      source = '完整报文';
    } else {
      const tx = record.match(TX_FRAME);
      if (!tx) {
        continue;
      }
      hex = tx[1];
      dir = 'tx';
      source = '帧=';
    }

    const warnings: string[] = [];
    if (hex.length % 2 !== 0) {
      warnings.push(`HEX 长度为奇数（${hex.length}），可能被截断`);
    }
    const bytes = hexToBytes(hex);
    if (bytes.length === 0 && warnings.length === 0) {
      continue;
    }
    if (declaredLen !== undefined && declaredLen !== bytes.length) {
      warnings.push(
        `日志声明 ${declaredLen} 字节，实际提取 ${bytes.length} 字节`,
      );
    }

    frames.push({
      hex: bytes.length > 0 ? formatHex(bytes) : formatLoose(hex),
      byteLen: bytes.length,
      dir,
      time,
      declaredLen,
      source,
      warning: warnings.join('；'),
    });
  }

  return frames;
}
