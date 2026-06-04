// SPDX-License-Identifier: GPL-2.0

//! Read-only HTTP + SSE state server for a running [`Campaign`].
//!
//! A frontend polls the JSON endpoints for snapshots and subscribes to
//! `/events` for live updates. All state lives in the shared [`Campaign`]; this
//! module is pure transport. Streaming uses Server-Sent Events.
//!
//! Endpoints (all GET): `/stats`, `/tree` (`?format=ascii|dot`),
//! `/corpus[?since=N]`, `/corpus/{id}`, `/solutions[?since=N]`,
//! `/solutions/{id}`, `/events`, `/` (dashboard).

use std::io::Write;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;

use tiny_http::{Header, Method, Request, Response, Server};

use crate::campaign::Campaign;

/// The dashboard, baked in and served at `/` so the API and UI are same-origin.
const INDEX_HTML: &str = include_str!("../web/index.html");

const ENDPOINTS: &str = r#"{"endpoints":["/stats","/tree","/tree?format=ascii","/tree?format=dot","/corpus","/corpus/{id}","/solutions","/solutions/{id}","/events"]}"#;

/// Serve the campaign's state API on `addr`. Blocks the calling thread; each
/// request is handled on its own thread so long-lived SSE streams don't block
/// other queries.
pub fn serve(campaign: Arc<Campaign>, addr: String) {
    let server = match Server::http(addr.as_str()) {
        Ok(s) => s,
        Err(e) => {
            crate::ui::warn(&format!("http: failed to bind {addr}: {e}"));
            return;
        }
    };
    crate::ui::info(&format!("serving state API on http://{addr}"));
    for request in server.incoming_requests() {
        let campaign = Arc::clone(&campaign);
        thread::spawn(move || handle(&campaign, request));
    }
}

fn handle(campaign: &Campaign, request: Request) {
    if *request.method() != Method::Get {
        let _ = request.respond(Response::from_string("method not allowed").with_status_code(405));
        return;
    }
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));

    match path {
        "/events" => serve_sse(campaign, request),
        "/stats" => respond_json(request, campaign.stats_json()),
        "/tree" => match query_get(query, "format").as_deref() {
            Some("ascii") => respond_text(request, campaign.tree_ascii()),
            Some("dot") => respond_text(request, campaign.tree_dot()),
            _ => respond_json(request, campaign.tree_json()),
        },
        "/corpus" => respond_json(request, campaign.corpus_json(since(query))),
        "/solutions" => respond_json(request, campaign.solutions_json(since(query))),
        "/" => respond_html(request, INDEX_HTML),
        "/api" => respond_json(request, ENDPOINTS.to_string()),
        _ => {
            if let Some(rest) = path.strip_prefix("/corpus/") {
                match rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|id| campaign.corpus_entry_json(id))
                {
                    Some(body) => respond_json(request, body),
                    None => respond_404(request),
                }
            } else if let Some(rest) = path.strip_prefix("/solutions/") {
                match rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|id| campaign.solution_json(id))
                {
                    Some(body) => respond_json(request, body),
                    None => respond_404(request),
                }
            } else {
                respond_404(request);
            }
        }
    }
}

fn serve_sse(campaign: &Campaign, request: Request) {
    let rx: Receiver<Vec<u8>> = campaign.subscribe();
    // Take over the raw socket: tiny_http's buffered Response path would never
    // flush a never-returning SSE write, so we write the head and flush frames.
    let mut w = request.into_writer();
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Cache-Control: no-cache\r\n\
                Access-Control-Allow-Origin: *\r\n\
                Connection: close\r\n\r\n";
    let init = format!(
        ": connected\n\nevent: stats\ndata: {}\n\n",
        campaign.stats_json()
    );
    if write_flush(&mut w, head.as_bytes()).is_err()
        || write_flush(&mut w, init.as_bytes()).is_err()
    {
        return;
    }
    while let Ok(frame) = rx.recv() {
        if write_flush(&mut w, &frame).is_err() {
            break;
        }
    }
}

fn write_flush<W: Write>(w: &mut W, bytes: &[u8]) -> std::io::Result<()> {
    w.write_all(bytes)?;
    w.flush()
}

fn since(query: &str) -> usize {
    query_get(query, "since")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn query_get(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header valid")
}

fn respond_html(request: Request, body: &str) {
    let resp =
        Response::from_string(body).with_header(header("Content-Type", "text/html; charset=utf-8"));
    let _ = request.respond(resp);
}

fn respond_json(request: Request, body: String) {
    let resp = Response::from_string(body)
        .with_header(header("Content-Type", "application/json"))
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = request.respond(resp);
}

fn respond_text(request: Request, body: String) {
    let resp = Response::from_string(body)
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = request.respond(resp);
}

fn respond_404(request: Request) {
    let resp = Response::from_string("not found")
        .with_status_code(404)
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = request.respond(resp);
}
