//! A tiny in-process HTTP/1.1 stub. One request per connection, `Connection: close`.

#![allow(dead_code, unreachable_pub)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    /// Path including the query string.
    pub target: String,
    /// Lowercased names.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Req {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    pub fn query(&self) -> &str {
        self.target.split_once('?').map_or("", |(_, q)| q)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Every header line as text, for hostile echo bodies.
    pub fn header_dump(&self) -> String {
        self.headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Close the connection without answering.
    pub hangup: bool,
}

impl Resp {
    pub fn json(status: u16, v: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: v.to_string().into_bytes(),
            hangup: false,
        }
    }

    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            hangup: false,
        }
    }

    pub fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
            hangup: false,
        }
    }

    pub fn hangup() -> Self {
        Self {
            status: 0,
            headers: Vec::new(),
            body: Vec::new(),
            hangup: true,
        }
    }

    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

pub struct Stub {
    base: String,
    requests: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
    handle: Option<JoinHandle<()>>,
}

type Handler = dyn Fn(&Req, &str) -> Resp + Send + Sync + 'static;

impl Stub {
    /// `handler(request, base_url)` answers each request.
    pub fn start(handler: impl Fn(&Req, &str) -> Resp + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let base = format!("http://{addr}");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let (r2, s2, b2) = (requests.clone(), stop.clone(), base.clone());
        let handle = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if s2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                let (h, r, b) = (handler.clone(), r2.clone(), b2.clone());
                std::thread::spawn(move || serve(stream, &*h, &r, &b));
            }
        });
        Self {
            base,
            requests,
            stop,
            addr,
            handle: Some(handle),
        }
    }

    pub fn url(&self) -> &str {
        &self.base
    }

    pub fn requests(&self) -> Vec<Req> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Req {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn serve(mut stream: TcpStream, handler: &Handler, log: &Mutex<Vec<Req>>, base: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(req) = read_request(&mut stream) else {
        return;
    };
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(req.clone());
    let resp = handler(&req, base);
    if resp.hangup {
        return;
    }
    let mut out = format!(
        "HTTP/1.1 {} X\r\nConnection: close\r\nContent-Length: {}\r\n",
        resp.status,
        resp.body.len()
    );
    for (k, v) in &resp.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.write_all(&resp.body);
    let _ = stream.flush();
}
