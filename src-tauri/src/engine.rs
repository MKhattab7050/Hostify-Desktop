use serde_json::{Value, json};
use std::{collections::HashMap, sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}}, time::Duration};
use tauri::{AppHandle, Emitter};
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, net::windows::named_pipe::{ClientOptions, NamedPipeClient}, sync::{Mutex, oneshot}, time::{sleep, timeout, Instant}};

type Replies = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;
struct Connection {
    write: Mutex<tokio::io::WriteHalf<NamedPipeClient>>,
    alive: Arc<AtomicBool>,
    replies: Replies,
    reader: tokio::task::AbortHandle,
}
impl Drop for Connection { fn drop(&mut self) { self.reader.abort(); } }
impl Connection {
    async fn disconnect(&self) {
        self.alive.store(false, Ordering::Release);
        self.reader.abort();
        for (_, reply) in self.replies.lock().await.drain() {
            let _ = reply.send(Err("Hosting engine disconnected; check the operation status before retrying".into()));
        }
    }
}
pub struct EngineClient {
    connection: Mutex<Option<Arc<Connection>>>,
    next: AtomicU64,
    pub close_to_tray: AtomicBool,
    app: AppHandle,
}
impl EngineClient {
    pub fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self { connection: Mutex::new(None), next: AtomicU64::new(1), close_to_tray: AtomicBool::new(true), app })
    }
    async fn connect(&self) -> Result<Arc<Connection>, String> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref().filter(|c| c.alive.load(Ordering::Acquire)) { return Ok(connection.clone()); }
        *slot = None;
        let name = super::windows::pipe_name()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut spawned = false;
        let pipe = loop {
            match ClientOptions::new().open(&name) {
                Ok(pipe) => break pipe,
                Err(e) => {
                    if !spawned && e.kind() == std::io::ErrorKind::NotFound {
                        super::windows::spawn_engine()?;
                        spawned = true;
                    }
                    if Instant::now() >= deadline { return Err(format!("Hosting engine is unavailable: {e}")); }
                    sleep(Duration::from_millis(150)).await;
                }
            }
        };
        let (read, mut write) = tokio::io::split(pipe);
        timeout(Duration::from_secs(5), write.write_all(b"{\"id\":0,\"method\":\"subscribe\",\"params\":{}}\n"))
            .await.map_err(|_| "Engine subscription timed out".to_string())?.map_err(|e| e.to_string())?;
        let replies: Replies = Arc::new(Mutex::new(HashMap::new()));
        let reader_replies = replies.clone();
        let app = self.app.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        let reader_task = tokio::spawn(async move {
            let mut reader = BufReader::new(read);
            loop {
                // Read a bounded frame, including while idle so pushed events arrive immediately.
                let mut bytes = Vec::new();
                let read = (&mut reader).take(4 * 1024 * 1024 + 1).read_until(b'\n', &mut bytes).await;
                match read { Ok(0) | Err(_) => break, _ => {} }
                if bytes.len() > 4 * 1024 * 1024 || bytes.last() != Some(&b'\n') { break; }
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { break; };
                if let Some(event) = value.get("event").and_then(Value::as_str) {
                    if matches!(event, "site-status" | "deployment-update" | "activity" | "components-progress") {
                        let _ = app.emit(event, value.get("payload").cloned().unwrap_or(Value::Null));
                    }
                } else if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    if let Some(reply) = reader_replies.lock().await.remove(&id) {
                        let result = if let Some(error) = value.get("error").and_then(Value::as_str) { Err(error.to_owned()) }
                        else if let Some(result) = value.get("result") { Ok(result.clone()) }
                        else { Err("Invalid engine response".into()) };
                        let _ = reply.send(result);
                    }
                }
            }
            reader_alive.store(false, Ordering::Release);
            for (_, reply) in reader_replies.lock().await.drain() {
                let _ = reply.send(Err("Hosting engine disconnected; check the operation status before retrying".into()));
            }
        });
        let connection = Arc::new(Connection { write: Mutex::new(write), alive, replies, reader: reader_task.abort_handle() });
        *slot = Some(connection.clone());
        Ok(connection)
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let connection = timeout(Duration::from_secs(12), self.connect()).await
            .map_err(|_| "Hosting engine connection timed out".to_string())??;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let request = async {
            let (tx, rx) = oneshot::channel();
            let mut bytes = serde_json::to_vec(&json!({"id": id, "method": method, "params": params})).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            let mut writer = connection.write.lock().await;
            if !connection.alive.load(Ordering::Acquire) { return Err("Hosting engine disconnected".into()); }
            connection.replies.lock().await.insert(id, tx);
            if let Err(e) = writer.write_all(&bytes).await {
                connection.disconnect().await;
                return Err(format!("Cannot send engine request: {e}"));
            }
            drop(writer);
            rx.await.map_err(|_| "Hosting engine connection closed".to_string())?
        };
        // Never replay mutations after an ambiguous failure. Replies belong to a
        // connection, so a late old response cannot affect a reconnected client.
        let result = match timeout(Duration::from_secs(120), request).await {
            Ok(result) => result,
            Err(_) => {
                connection.disconnect().await;
                Err("Hosting engine request timed out; check the operation status before retrying".into())
            }
        };
        connection.replies.lock().await.remove(&id);
        if let Ok(settings) = &result {
            if matches!(method, "get_settings" | "save_settings") {
                self.close_to_tray.store(settings.get("closeToTray").and_then(Value::as_bool).unwrap_or(true), Ordering::Relaxed);
            }
        }
        result
    }
}
