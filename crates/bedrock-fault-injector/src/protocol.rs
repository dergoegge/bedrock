//! Wire protocol and shared constants for the client/server.
//!
//! One request per connection, one response back, both newline-delimited JSON.

use serde::{Deserialize, Serialize};

/// Pathname unix-domain socket the server listens on and clients connect to.
/// It lives in the guest's host netns (under /run, a tmpfs), so it is immune to
/// the container-network faults this tool installs.
pub const SOCKET_PATH: &str = "/run/bedrock-fault.sock";

/// A command sent from a client to the server.
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Drop traffic to/from a container. With `peer`, only traffic between the
    /// two is dropped; otherwise the container is isolated from all peers.
    /// `duration_ms`, when set, auto-expires the fault after that many
    /// milliseconds of guest time; `None` means it persists until `Clear`.
    Partition {
        container: String,
        peer: Option<String>,
        duration_ms: Option<u64>,
    },
    /// Remove every fault the server currently tracks.
    Clear,
}

/// The server's reply.
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Err(String),
}

/// Parse a human duration into milliseconds: `500ms`, `5s`, `2m`, or a bare
/// number (interpreted as seconds). Used by the client's `--duration` parser.
pub fn parse_duration_ms(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1u64)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else {
        (s, 1000) // bare number => seconds
    };
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration: {s:?}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("duration must be a non-negative number: {s:?}"));
    }
    Ok((value * mult as f64).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::parse_duration_ms;

    #[test]
    fn parses_duration_units() {
        assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
        assert_eq!(parse_duration_ms("5s").unwrap(), 5_000);
        assert_eq!(parse_duration_ms("2m").unwrap(), 120_000);
        assert_eq!(parse_duration_ms("90").unwrap(), 90_000); // bare => seconds
        assert_eq!(parse_duration_ms("1.5s").unwrap(), 1_500);
        assert!(parse_duration_ms("abc").is_err());
        assert!(parse_duration_ms("-3s").is_err());
    }
}
