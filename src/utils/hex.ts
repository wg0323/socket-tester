/** HEX / ASCII 与字节数组的互转，以及发送框的容错解析。 */

export interface HexParseResult {
  bytes: number[];
  error: string;
}

const SEPARATORS = /[\s,;:\-]/g;
const ZERO_X = /0x/gi;
const ILLEGAL = /[^0-9a-fA-F]/;

/** 把 "7E 01 0A"、"0x7E,0x01"、"7E010A" 等写法统一解析成字节。 */
export function parseHexText(text: string): HexParseResult {
  const cleaned = text.replace(ZERO_X, '').replace(SEPARATORS, '');
  if (cleaned.length === 0) {
    return { bytes: [], error: '报文内容为空' };
  }
  const illegal = cleaned.match(ILLEGAL);
  if (illegal) {
    return { bytes: [], error: `包含非法 HEX 字符: ${illegal[0]}` };
  }
  if (cleaned.length % 2 !== 0) {
    return {
      bytes: [],
      error: `HEX 字符数为奇数（${cleaned.length}），无法组成完整字节`,
    };
  }
  const bytes: number[] = [];
  for (let i = 0; i < cleaned.length; i += 2) {
    bytes.push(Number.parseInt(cleaned.slice(i, i + 2), 16));
  }
  return { bytes, error: '' };
}

/** 字节数组转成大写、空格分隔的 HEX 文本。 */
export function formatHex(bytes: number[]): string {
  return bytes
    .map((byte) => byte.toString(16).toUpperCase().padStart(2, '0'))
    .join(' ');
}

/** 宽松解析，失败返回空数组，用于接收日志的展示。 */
export function hexToBytes(hex: string): number[] {
  return parseHexText(hex).bytes;
}

/** 非可打印字节显示为点，避免日志列表排版错乱。 */
export function bytesToAscii(bytes: number[]): string {
  let out = '';
  for (const byte of bytes) {
    out += byte >= 0x20 && byte <= 0x7e ? String.fromCharCode(byte) : '.';
  }
  return out;
}

/** ASCII 发送时按 UTF-8 编码成字节。 */
export function asciiToBytes(text: string): number[] {
  return Array.from(new TextEncoder().encode(text));
}
