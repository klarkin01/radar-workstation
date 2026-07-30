//! `#[cfg(test)]`-only scripted plaintext TCP server (D-d). Never compiled
//! into a release build; `Client` never speaks plaintext.

#![cfg(test)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub(crate) enum Action {
    Write(Vec<u8>),
    /// Sleep past the test's configured timeout, then drop the connection.
    StallThenClose(Duration),
    Close,
}

/// One inner `Vec<Action>` per request the server expects to receive on a
/// given connection, in order.
pub(crate) type ConnectionScript = Vec<Vec<Action>>;

pub(crate) struct TestServer {
    pub(crate) addr: SocketAddr,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    accept_count: Arc<AtomicUsize>,
}

impl TestServer {
    /// Starts a listener that accepts exactly `scripts.len()` connections,
    /// running each connection's script in order.
    pub(crate) async fn start(scripts: Vec<ConnectionScript>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener should bind");
        let addr = listener.local_addr().expect("listener should report an address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let accept_count = Arc::new(AtomicUsize::new(0));

        let requests_task = requests.clone();
        let accept_count_task = accept_count.clone();
        tokio::spawn(async move {
            for script in scripts {
                let (stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => return,
                };
                accept_count_task.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(handle_connection(stream, requests_task.clone(), script));
            }
        });

        Self { addr, requests, accept_count }
    }

    pub(crate) fn requests(&self) -> Vec<Vec<u8>> {
        self.requests.lock().expect("mutex not poisoned").clone()
    }

    pub(crate) fn accept_count(&self) -> usize {
        self.accept_count.load(Ordering::SeqCst)
    }
}

async fn handle_connection(mut stream: TcpStream, requests: Arc<Mutex<Vec<Vec<u8>>>>, script: ConnectionScript) {
    for actions in script {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            match stream.read(&mut tmp).await {
                Ok(0) => {
                    requests.lock().expect("mutex not poisoned").push(buf);
                    return;
                }
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
        requests.lock().expect("mutex not poisoned").push(buf);

        for action in actions {
            match action {
                Action::Write(bytes) => {
                    if stream.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
                Action::StallThenClose(dur) => {
                    tokio::time::sleep(dur).await;
                    return;
                }
                Action::Close => return,
            }
        }
    }
}
