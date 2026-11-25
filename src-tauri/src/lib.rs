//! TCP / UDP 报文收发与重放。
//!
//! 后端只负责搬运字节：连接管理、收发、把日志批量推给前端。
//! 报文解析、HEX 清洗、日志提取全部在前端完成，后端不做任何协议假设。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, UdpSocket};
use tokio::sync::mpsc;

const READ_BUF: usize = 16 * 1024;
const LOG_FLUSH_MS: u64 = 50;

static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------- 基础工具

fn now_ts() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{b:02X}");
    }
    out
}

fn new_session_id() -> String {
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("s{}-{seq}", Local::now().timestamp_millis())
}

fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text
        .replace("0x", "")
        .replace("0X", "")
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, ',' | ':' | ';' | '-'))
        .collect();

    if cleaned.is_empty() {
        return Err("报文内容为空".into());
    }
    if cleaned.len() % 2 != 0 {
        return Err(format!("HEX 字符数为奇数（{}），无法组成完整字节", cleaned.len()));
    }

    let bytes = cleaned.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char)
            .to_digit(16)
            .ok_or_else(|| format!("非法 HEX 字符: {}", bytes[i] as char))?;
        let lo = (bytes[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| format!("非法 HEX 字符: {}", bytes[i + 1] as char))?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Ok(out)
}

fn bind_ip(local_ip: Option<&str>) -> Result<IpAddr, String> {
    match local_ip {
        Some(ip) if !ip.trim().is_empty() => ip
            .trim()
            .parse()
            .map_err(|_| format!("本地 IP 格式错误: {ip}")),
        _ => Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
    }
}

async fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    if let Ok(ip) = host.trim().parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    let mut addrs = tokio::net::lookup_host((host.trim(), port))
        .await
        .map_err(|e| format!("域名解析失败: {e}"))?;
    addrs.next().ok_or_else(|| format!("域名无解析结果: {host}"))
}

// ---------------------------------------------------------------- 日志推送

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEvent {
    ts: String,
    dir: String,
    session_id: String,
    peer: Option<String>,
    hex: String,
    len: usize,
}

/// 累积 50ms 后整批推给前端，避免高频报文把 IPC 打满。
#[derive(Clone)]
struct LogSink(std::sync::mpsc::Sender<LogEvent>);

impl LogSink {
    fn new(app: AppHandle) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<LogEvent>();
        std::thread::spawn(move || {
            let mut buf: Vec<LogEvent> = Vec::new();
            loop {
                match rx.recv() {
                    Ok(ev) => buf.push(ev),
                    Err(_) => break,
                }
                let deadline = Instant::now() + Duration::from_millis(LOG_FLUSH_MS);
                loop {
                    let remain = deadline.saturating_duration_since(Instant::now());
                    if remain.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(remain) {
                        Ok(ev) => buf.push(ev),
                        Err(_) => break,
                    }
                }
                if !buf.is_empty() {
                    let batch: Vec<LogEvent> = buf.drain(..).collect();
                    let _ = app.emit("log:batch", batch);
                }
            }
        });
        Self(tx)
    }

    fn push(&self, ev: LogEvent) {
        let _ = self.0.send(ev);
    }

    fn frame(&self, session_id: &str, dir: &str, peer: Option<String>, hex: String, len: usize) {
        self.push(LogEvent {
            ts: now_ts(),
            dir: dir.into(),
            session_id: session_id.into(),
            peer,
            hex,
            len,
        });
    }

    fn data(&self, session_id: &str, dir: &str, peer: Option<String>, bytes: &[u8]) {
        self.frame(session_id, dir, peer, to_hex(bytes), bytes.len());
    }

    fn sys(&self, session_id: &str, message: &str) {
        self.push(LogEvent {
            ts: now_ts(),
            dir: "sys".into(),
            session_id: session_id.into(),
            peer: None,
            hex: message.into(),
            len: 0,
        });
    }
}

// ---------------------------------------------------------------- 会话

type ClientMap = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Vec<u8>>>>>;

enum Writer {
    /// TCP Client：单条上行通道
    Stream(mpsc::UnboundedSender<Vec<u8>>),
    /// TCP Server：多客户端，可按对端定向发送或广播
    Clients(ClientMap),
    /// UDP：无连接，按目标地址发送
    Datagram {
        socket: Arc<UdpSocket>,
        last_peer: Arc<Mutex<Option<SocketAddr>>>,
    },
}

struct Session {
    writer: Writer,
    stop: Arc<AtomicBool>,
}

struct AppState {
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    sink: LogSink,
}

impl AppState {
    fn new(app: AppHandle) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            sink: LogSink::new(app),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectRequest {
    kind: String,
    host: Option<String>,
    port: Option<u16>,
    local_ip: Option<String>,
    local_port: Option<u16>,
    multicast: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionInfo {
    id: String,
    kind: String,
    local: String,
    peers: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NetIf {
    name: String,
    ip: String,
}

// ---------------------------------------------------------------- 命令

#[tauri::command]
async fn connect(
    state: State<'_, AppState>,
    req: ConnectRequest,
) -> Result<SessionInfo, String> {
    let sessions = state.sessions.clone();
    let sink = state.sink.clone();
    let id = new_session_id();

    match req.kind.as_str() {
        "tcp-client" => {
            let host = req.host.unwrap_or_default();
            let port = req.port.ok_or("缺少目的端口")?;
            if host.trim().is_empty() {
                return Err("缺少目的地址".into());
            }
            let remote = resolve(&host, port).await?;
            let local_port = req.local_port.unwrap_or(0);

            let socket = TcpSocket::new_v4().map_err(|e| format!("创建套接字失败: {e}"))?;
            socket
                .bind(SocketAddr::new(bind_ip(req.local_ip.as_deref())?, local_port))
                .map_err(|e| format!("绑定本地地址失败: {e}"))?;
            let stream = socket
                .connect(remote)
                .await
                .map_err(|e| format!("连接 {remote} 失败: {e}"))?;

            let local = stream
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let peer = stream
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| remote.to_string());

            let (mut rd, mut wr) = stream.into_split();
            let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();

            tauri::async_runtime::spawn(async move {
                while let Some(data) = rx.recv().await {
                    if wr.write_all(&data).await.is_err() {
                        break;
                    }
                    let _ = wr.flush().await;
                }
                let _ = wr.shutdown().await;
            });

            let sid = id.clone();
            let peer_rx = peer.clone();
            let sink_rx = sink.clone();
            tauri::async_runtime::spawn(async move {
                let mut buf = vec![0u8; READ_BUF];
                loop {
                    match rd.read(&mut buf).await {
                        Ok(0) => {
                            sink_rx.sys(&sid, "对端已关闭连接");
                            break;
                        }
                        Ok(n) => sink_rx.data(&sid, "rx", Some(peer_rx.clone()), &buf[..n]),
                        Err(e) => {
                            sink_rx.sys(&sid, &format!("接收失败: {e}"));
                            break;
                        }
                    }
                }
            });

            sessions.lock().unwrap().insert(
                id.clone(),
                Arc::new(Session {
                    writer: Writer::Stream(tx),
                    stop: Arc::new(AtomicBool::new(false)),
                }),
            );
            sink.sys(&id, &format!("已连接 {peer}"));

            Ok(SessionInfo {
                id,
                kind: "tcp-client".into(),
                local,
                peers: vec![peer],
            })
        }

        "tcp-server" => {
            let local_port = req.local_port.unwrap_or(0);
            let listener = TcpListener::bind(SocketAddr::new(
                bind_ip(req.local_ip.as_deref())?,
                local_port,
            ))
            .await
            .map_err(|e| format!("监听失败: {e}"))?;

            let local = listener
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let clients: ClientMap = Arc::new(Mutex::new(HashMap::new()));
            let stop = Arc::new(AtomicBool::new(false));

            let sid = id.clone();
            let sink_srv = sink.clone();
            let clients_srv = clients.clone();
            let stop_srv = stop.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            if stop_srv.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                        accepted = listener.accept() => {
                            match accepted {
                                Ok((stream, addr)) => {
                                    let peer = addr.to_string();
                                    let (mut rd, mut wr) = stream.into_split();
                                    let (ctx, mut crx) = mpsc::unbounded_channel::<Vec<u8>>();
                                    clients_srv.lock().unwrap().insert(peer.clone(), ctx);
                                    sink_srv.sys(&sid, &format!("对端接入 {peer}"));

                                    tauri::async_runtime::spawn(async move {
                                        while let Some(data) = crx.recv().await {
                                            if wr.write_all(&data).await.is_err() {
                                                break;
                                            }
                                            let _ = wr.flush().await;
                                        }
                                        let _ = wr.shutdown().await;
                                    });

                                    let sid_rx = sid.clone();
                                    let peer_rx = peer.clone();
                                    let sink_rx = sink_srv.clone();
                                    let clients_rx = clients_srv.clone();
                                    tauri::async_runtime::spawn(async move {
                                        let mut buf = vec![0u8; READ_BUF];
                                        loop {
                                            match rd.read(&mut buf).await {
                                                Ok(0) => break,
                                                Ok(n) => sink_rx.data(
                                                    &sid_rx,
                                                    "rx",
                                                    Some(peer_rx.clone()),
                                                    &buf[..n],
                                                ),
                                                Err(_) => break,
                                            }
                                        }
                                        clients_rx.lock().unwrap().remove(&peer_rx);
                                        sink_rx.sys(&sid_rx, &format!("对端断开 {peer_rx}"));
                                    });
                                }
                                Err(e) => {
                                    sink_srv.sys(&sid, &format!("接受连接失败: {e}"));
                                }
                            }
                        }
                    }
                }
                sink_srv.sys(&sid, "监听已停止");
            });

            sessions.lock().unwrap().insert(
                id.clone(),
                Arc::new(Session {
                    writer: Writer::Clients(clients),
                    stop,
                }),
            );
            sink.sys(&id, &format!("开始监听 {local}"));

            Ok(SessionInfo {
                id,
                kind: "tcp-server".into(),
                local,
                peers: Vec::new(),
            })
        }

        "udp" => {
            let local_port = req.local_port.unwrap_or(0);
            let socket = UdpSocket::bind(SocketAddr::new(
                bind_ip(req.local_ip.as_deref())?,
                local_port,
            ))
            .await
            .map_err(|e| format!("绑定本地端口失败: {e}"))?;

            if let Some(group) = req.multicast.as_deref().filter(|s| !s.trim().is_empty()) {
                let group: Ipv4Addr = group
                    .trim()
                    .parse()
                    .map_err(|_| format!("组播组地址格式错误: {group}"))?;
                socket
                    .join_multicast_v4(group, Ipv4Addr::UNSPECIFIED)
                    .map_err(|e| format!("加入组播组失败: {e}"))?;
            }

            let local = socket
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let socket = Arc::new(socket);
            let last_peer: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
            let stop = Arc::new(AtomicBool::new(false));

            let sid = id.clone();
            let sink_rx = sink.clone();
            let socket_rx = socket.clone();
            let last_peer_rx = last_peer.clone();
            let stop_rx = stop.clone();
            tauri::async_runtime::spawn(async move {
                let mut buf = vec![0u8; READ_BUF];
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            if stop_rx.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                        got = socket_rx.recv_from(&mut buf) => {
                            match got {
                                Ok((n, addr)) => {
                                    *last_peer_rx.lock().unwrap() = Some(addr);
                                    sink_rx.data(&sid, "rx", Some(addr.to_string()), &buf[..n]);
                                }
                                Err(e) => sink_rx.sys(&sid, &format!("接收失败: {e}")),
                            }
                        }
                    }
                }
            });

            sessions.lock().unwrap().insert(
                id.clone(),
                Arc::new(Session {
                    writer: Writer::Datagram {
                        socket,
                        last_peer,
                    },
                    stop,
                }),
            );
            sink.sys(&id, &format!("UDP 已绑定 {local}"));

            Ok(SessionInfo {
                id,
                kind: "udp".into(),
                local,
                peers: Vec::new(),
            })
        }

        other => Err(format!("不支持的模式: {other}")),
    }
}

#[tauri::command]
async fn send_data(
    state: State<'_, AppState>,
    session_id: String,
    hex: String,
    target: Option<String>,
) -> Result<usize, String> {
    let (session, sink) = {
        let map = state.sessions.lock().unwrap();
        let session = map
            .get(&session_id)
            .cloned()
            .ok_or_else(|| "会话不存在或已断开".to_string())?;
        (session, state.sink.clone())
    };

    let bytes = parse_hex(&hex)?;
    let len = bytes.len();
    let normalized = to_hex(&bytes);
    let log_target = target.clone();

    match &session.writer {
        Writer::Stream(tx) => {
            tx.send(bytes).map_err(|_| "连接已关闭".to_string())?;
        }
        Writer::Clients(clients) => {
            let mut map = clients.lock().unwrap();
            match log_target.as_deref().filter(|t| !t.trim().is_empty()) {
                Some(peer) => {
                    let tx = map
                        .get(peer)
                        .ok_or_else(|| format!("对端不存在: {peer}"))?;
                    tx.send(bytes).map_err(|_| "对端连接已关闭".to_string())?;
                }
                None => {
                    map.retain(|_, tx| tx.send(bytes.clone()).is_ok());
                }
            }
        }
        Writer::Datagram { socket, last_peer } => {
            let peer: SocketAddr = match log_target.as_deref().filter(|t| !t.trim().is_empty()) {
                Some(t) => t
                    .trim()
                    .parse()
                    .map_err(|_| format!("目标地址格式错误: {t}"))?,
                None => last_peer
                    .lock()
                    .unwrap()
                    .ok_or_else(|| "尚未收到数据，请填写目的 IP 和端口".to_string())?,
            };
            socket
                .send_to(&bytes, peer)
                .await
                .map_err(|e| format!("发送失败: {e}"))?;
        }
    }

    sink.frame(
        &session_id,
        "tx",
        log_target.filter(|t| !t.trim().is_empty()),
        normalized,
        len,
    );
    Ok(len)
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>, session_id: String) {
    if let Some(session) = state.sessions.lock().unwrap().remove(&session_id) {
        session.stop.store(true, Ordering::SeqCst);
        state.sink.sys(&session_id, "已断开");
    }
}

// ---------------------------------------------------------------- 工程存储

/// 工程统一存放于应用数据目录，用户不需要选择保存位置。
fn projects_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取数据目录失败: {e}"))?
        .join("projects");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建工程目录失败: {e}"))?;
    Ok(dir)
}

fn is_valid_project_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn project_path(app: &AppHandle, id: &str) -> Result<std::path::PathBuf, String> {
    if !is_valid_project_id(id) {
        return Err("工程标识不合法".into());
    }
    Ok(projects_dir(app)?.join(format!("{id}.json")))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSummary {
    id: String,
    name: String,
    env_count: usize,
    message_count: usize,
    updated_at: u64,
}

#[tauri::command]
fn list_projects(app: AppHandle) -> Result<Vec<ProjectSummary>, String> {
    let dir = projects_dir(&app)?;
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("读取工程目录失败: {e}"))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        if !is_valid_project_id(&id) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let updated_at = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        out.push(ProjectSummary {
            id,
            name: value
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("未命名工程")
                .to_string(),
            env_count: value
                .get("environments")
                .and_then(|v| v.as_object())
                .map_or(0, |o| o.len()),
            message_count: value
                .get("messages")
                .and_then(|v| v.as_array())
                .map_or(0, |a| a.len()),
            updated_at,
        });
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

#[tauri::command]
fn read_project(app: AppHandle, id: String) -> Result<String, String> {
    let path = project_path(&app, &id)?;
    std::fs::read_to_string(&path).map_err(|e| format!("读取工程失败: {e}"))
}

#[tauri::command]
fn write_project(app: AppHandle, id: String, content: String) -> Result<(), String> {
    let path = project_path(&app, &id)?;
    std::fs::write(&path, content).map_err(|e| format!("保存工程失败: {e}"))
}

#[tauri::command]
fn delete_project(app: AppHandle, id: String) -> Result<(), String> {
    let path = project_path(&app, &id)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("删除工程失败: {e}"))?;
    }
    Ok(())
}

/// 仅用于导入旧的工程文件。
#[tauri::command]
fn read_project_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("读取工程失败: {e}"))
}

#[tauri::command]
fn write_text_file(path: String, content: String) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
        }
    }
    std::fs::write(&path, content).map_err(|e| format!("写入文件失败: {e}"))
}

#[tauri::command]
fn list_local_ips() -> Vec<NetIf> {
    let mut out = Vec::new();
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for it in ifs {
            if let IpAddr::V4(v4) = it.addr.ip() {
                if v4.is_loopback() {
                    continue;
                }
                out.push(NetIf {
                    name: it.name.clone(),
                    ip: v4.to_string(),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------- 入口

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(AppState::new(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            send_data,
            disconnect,
            list_local_ips,
            list_projects,
            read_project,
            write_project,
            delete_project,
            read_project_file,
            write_text_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
