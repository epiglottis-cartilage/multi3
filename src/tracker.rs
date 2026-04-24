use std::{
    collections::HashMap,
    sync::atomic::AtomicU64,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ConnectionStatus {
    Waiting,
    Connected,
    CompletedError,
    CompletedNormal,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Protocol {
    Http,
    Https,
    Socks5Tcp,
    Socks5Udp,
}

#[derive(Clone, Debug)]
pub struct Connection {
    pub id: u64,
    pub start_time: Instant,
    pub upload_bytes: Arc<AtomicU64>,
    pub download_bytes: Arc<AtomicU64>,
    pub status: ConnectionStatus,
    pub local_addr: String,
    pub remote_uri: String,
    pub protocol: Protocol,
    pub retries: Vec<String>,
    pub completed_at: Option<Instant>,
    pub error_details: Option<String>,
}

#[derive(Clone)]
pub struct Tracker {
    inner: Arc<Mutex<TrackerInner>>,
}

struct TrackerInner {
    connections: HashMap<u64, Connection>,
    order: Vec<u64>,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(TrackerInner {
                connections: HashMap::new(),
                order: Vec::new(),
            })),
        }
    }

    pub fn add(&self, id: u64, local_addr: String, remote_uri: String, protocol: Protocol) {
        let mut inner = self.inner.lock().unwrap();
        let conn: Connection = Connection {
            id,
            start_time: Instant::now(),
            upload_bytes: Arc::new(AtomicU64::new(0)),
            download_bytes: Arc::new(AtomicU64::new(0)),
            status: ConnectionStatus::Waiting,
            local_addr,
            remote_uri,
            protocol,
            retries: Vec::new(),
            completed_at: None,
            error_details: None,
        };
        inner.connections.insert(id, conn);
        inner.order.insert(0, id);
    }

    pub fn set_connected(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(conn) = inner.connections.get_mut(&id) {
            conn.status = ConnectionStatus::Connected;
        }
    }

    pub fn add_retry(&self, id: u64, error: String) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(conn) = inner.connections.get_mut(&id) {
            conn.retries.push(error);
        }
    }

    pub fn set_completed(&self, id: u64, error: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(conn) = inner.connections.get_mut(&id) {
            conn.status = if error.is_some() {
                ConnectionStatus::CompletedError
            } else {
                ConnectionStatus::CompletedNormal
            };
            conn.error_details = error;
            conn.completed_at = Some(Instant::now());
        }
    }

    pub fn update_info(&self, id: u64, remote_uri: String, protocol: Protocol) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(conn) = inner.connections.get_mut(&id) {
            conn.remote_uri = remote_uri;
            conn.protocol = protocol;
        }
    }

    pub fn update_local_addr(&self, id: u64, local_ip: String) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(conn) = inner.connections.get_mut(&id) {
            conn.local_addr = local_ip;
        }
    }

    pub fn get_connections(&self) -> Vec<Connection> {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        let to_remove: Vec<u64> = inner
            .connections
            .values()
            .filter(|c| {
                c.completed_at
                    .map(|t| now.duration_since(t) > Duration::from_secs(3))
                    .unwrap_or(false)
            })
            .map(|c| c.id)
            .collect();

        for id in &to_remove {
            inner.connections.remove(id);
            inner.order.retain(|&x| x != *id);
        }

        inner
            .order
            .iter()
            .filter_map(|id| inner.connections.get(id).cloned())
            .collect()
    }
}
