use std::{
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
    connections: Arc<Mutex<Vec<Connection>>>,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub fn add(&self, id: u64, local_addr: String, remote_uri: String, protocol: Protocol) {
        let mut connections = self.connections.lock().unwrap();
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
        connections.push(conn);
    }

    pub fn set_connected(&self, id: u64) {
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn) = connections
            .binary_search_by_key(&id, |c| c.id)
            .ok()
            .map(|i| &mut connections[i])
        {
            conn.status = ConnectionStatus::Connected;
        }
    }

    pub fn add_retry(&self, id: u64, error: String) {
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn) = connections
            .binary_search_by_key(&id, |c| c.id)
            .ok()
            .map(|i| &mut connections[i])
        {
            conn.retries.push(error);
        }
    }

    pub fn set_completed(&self, id: u64, error: Option<String>) {
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn) = connections
            .binary_search_by_key(&id, |c| c.id)
            .ok()
            .map(|i| &mut connections[i])
        {
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
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn) = connections
            .binary_search_by_key(&id, |c| c.id)
            .ok()
            .map(|i| &mut connections[i])
        {
            conn.remote_uri = remote_uri;
            conn.protocol = protocol;
        }
    }

    pub fn update_local_addr(&self, id: u64, local_ip: String) {
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn) = connections
            .binary_search_by_key(&id, |c| c.id)
            .ok()
            .map(|i| &mut connections[i])
        {
            conn.local_addr = local_ip;
        }
    }

    pub fn clean_connections(&self) {
        let mut connections = self.connections.lock().unwrap();
        let now = Instant::now();
        connections.retain(|c| {
            c.completed_at
                .map(|t| now.duration_since(t) <= Duration::from_secs(3))
                .unwrap_or(true)
        });
    }
    pub fn with_connections<F: FnOnce(&mut [Connection]) -> R, R>(&self, f: F) -> R {
        let mut connections = self.connections.lock().unwrap();
        f(connections.as_mut_slice())
    }
}
