//! Test-only helpers: a tiny scriptable HTTP server (downloads, LLM endpoints, health checks).
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Debug, Default)]
pub struct Logged {
    pub method: String,
    pub url: String,
    pub range: Option<String>,
    pub body: String,
}

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl Reply {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self { status: 200, body: body.into(), headers: vec![] }
    }
    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self { status, body: body.into(), headers: vec![] }
    }
    pub fn json(v: serde_json::Value) -> Self {
        Self { status: 200, body: v.to_string().into_bytes(), headers: vec![("Content-Type".into(), "application/json".into())] }
    }
}

/// Serves `data` honouring `Range: bytes=N-` (206 + Content-Range), like a real file host.
pub fn range_reply(data: &[u8], range: Option<&str>) -> Reply {
    if let Some(start) = range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.strip_suffix('-')).and_then(|n| n.parse::<usize>().ok()) {
        if start >= data.len() {
            return Reply::status(416, "");
        }
        return Reply {
            status: 206,
            body: data[start..].to_vec(),
            headers: vec![("Content-Range".into(), format!("bytes {start}-{}/{}", data.len() - 1, data.len()))],
        };
    }
    Reply::ok(data.to_vec())
}

pub struct MockServer {
    pub base: String,
    pub log: Arc<Mutex<Vec<Logged>>>,
    server: Arc<tiny_http::Server>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    pub fn start(handler: impl Fn(&Logged) -> Reply + Send + Sync + 'static) -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let log = Arc::new(Mutex::new(vec![]));
        let (s2, l2) = (server.clone(), log.clone());
        let handle = std::thread::spawn(move || {
            for mut req in s2.incoming_requests() {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let range = req.headers().iter().find(|h| h.field.equiv("Range")).map(|h| h.value.to_string());
                let l = Logged { method: req.method().to_string(), url: req.url().to_string(), range, body };
                l2.lock().unwrap().push(l.clone());
                let r = handler(&l);
                let mut resp = tiny_http::Response::from_data(r.body).with_status_code(r.status);
                for (k, v) in r.headers {
                    resp.add_header(tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap());
                }
                let _ = req.respond(resp);
            }
        });
        Self { base, log, server, handle: Some(handle) }
    }

    pub fn requests(&self) -> Vec<Logged> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
