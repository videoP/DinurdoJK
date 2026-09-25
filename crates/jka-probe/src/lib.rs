//! Socket transport for a single explicit IPv4 server-info query.

use std::io;
use std::net::{SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jka_protocol::{getinfo_request, parse_info_response, ServerInfo};

static QUERY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The token correlates replies; it is not cryptographic authentication.
pub fn query_info(server: SocketAddrV4, timeout: Duration) -> io::Result<ServerInfo> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let token = format!(
        "{:016x}{:016x}",
        now.as_nanos() as u64,
        QUERY_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    query_with_token(server, timeout, &token)
}

fn query_with_token(
    server: SocketAddrV4,
    timeout: Duration,
    token: &str,
) -> io::Result<ServerInfo> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .filter(|_| !timeout.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid query timeout"))?;
    let packet =
        getinfo_request(token).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    // A connected UDP socket filters replies to this exact remote endpoint.
    socket.connect(server)?;
    socket.set_write_timeout(Some(timeout))?;
    socket.send(&packet)?;
    // Receive a whole UDP datagram before applying the much smaller protocol bound.
    let mut buffer = [0u8; 65535];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "no matching server-info response",
            ));
        }
        socket.set_read_timeout(Some(remaining))?;
        match socket.recv(&mut buffer) {
            Ok(len) => {
                if let Ok(info) = parse_info_response(&buffer[..len], token) {
                    return Ok(info);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no matching server-info response",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::thread;

    #[test]
    fn loopback_ignores_stale_and_malformed_responses() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let SocketAddr::V4(address) = server.local_addr().unwrap() else {
            unreachable!()
        };
        let worker = thread::spawn(move || {
            let mut request = [0; 128];
            let (len, client) = server.recv_from(&mut request).unwrap();
            assert_eq!(&request[..len], b"\xff\xff\xff\xffgetinfo test123");
            server.send_to(b"invalid", client).unwrap();
            server
                .send_to(
                    b"\xff\xff\xff\xffinfoResponse\n\\challenge\\stale\\protocol\\26",
                    client,
                )
                .unwrap();
            server.send_to(b"\xff\xff\xff\xffinfoResponse\n\\challenge\\test123\\protocol\\26\\hostname\\Local test", client).unwrap();
        });
        let info = query_with_token(address, Duration::from_secs(2), "test123").unwrap();
        worker.join().unwrap();
        assert_eq!(info.get(b"hostname"), Some(b"Local test".as_slice()));
    }

    #[test]
    fn silent_server_times_out() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let SocketAddr::V4(address) = server.local_addr().unwrap() else {
            unreachable!()
        };
        let error = query_info(address, Duration::from_millis(30)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            query_info(address, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
