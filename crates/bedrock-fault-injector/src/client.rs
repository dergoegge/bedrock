//! Thin client: connect to the server, send one request, print/return the
//! result.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::protocol::{Request, Response, SOCKET_PATH};

/// Send `req` to the server and translate its response into a process result.
pub fn run(req: &Request) -> Result<(), String> {
    let mut stream = connect()?;

    let mut payload = serde_json::to_string(req).map_err(|e| format!("serialize request: {e}"))?;
    payload.push('\n');
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| format!("send request: {e}"))?;
    // Half-close so the server sees EOF and reads a complete request.
    stream
        .shutdown(Shutdown::Write)
        .map_err(|e| format!("shutdown write: {e}"))?;

    let mut resp = String::new();
    stream
        .read_to_string(&mut resp)
        .map_err(|e| format!("read response: {e}"))?;
    match serde_json::from_str::<Response>(resp.trim())
        .map_err(|e| format!("parse response {resp:?}: {e}"))?
    {
        Response::Ok => Ok(()),
        Response::Err(msg) => Err(msg),
    }
}

/// Connect to the server's socket, retrying briefly. The server is started at
/// boot, but a fault action can race startup; the bounded retry (≈2s of guest
/// time) absorbs that without hanging forever if the server is truly absent.
fn connect() -> Result<UnixStream, String> {
    const MAX_ATTEMPTS: u32 = 40;
    for attempt in 1..=MAX_ATTEMPTS {
        match UnixStream::connect(SOCKET_PATH) {
            Ok(stream) => return Ok(stream),
            Err(e) if attempt == MAX_ATTEMPTS => {
                return Err(format!("cannot reach fault server at {SOCKET_PATH}: {e}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    unreachable!("loop returns on the final attempt")
}
