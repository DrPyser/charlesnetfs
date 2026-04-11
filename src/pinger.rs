use std::fmt::{Display, Formatter};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub struct PingResult {
    pub latency_ms: Option<u64>,
    pub payload: String,
}

impl Display for PingResult {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        let latency = self.latency_ms.unwrap();
        write!(f, "pong\t{}\n{}", latency, self.payload)?;
        Ok(())
    }
}

pub fn ping(target: &str, timeout_ms: u64, payload: &str) -> Result<PingResult, String> {
    let start = Instant::now();

    let stream_result = TcpStream::connect_timeout(
        &target.parse().map_err(|e| format!("{:?}", e))?,
        Duration::from_millis(timeout_ms),
    );

    match stream_result {
        Ok(_stream) => {
            let elapsed = start.elapsed().as_millis() as u64;
            Ok(PingResult {
                latency_ms: Some(elapsed),
                payload: payload.to_string(),
            })
        }
        Err(e) => Err(format!("{}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_ping_result_display() {
        let result = PingResult {
            latency_ms: Some(42),
            payload: "hello".to_string(),
        };
        let output = format!("{}", result);
        assert_eq!(output, "pong\t42\nhello");
    }

    #[test]
    fn test_ping_result_display_zero_latency() {
        let result = PingResult {
            latency_ms: Some(0),
            payload: "".to_string(),
        };
        let output = format!("{}", result);
        assert_eq!(output, "pong\t0\n");
    }

    #[test]
    fn test_ping_invalid_address() {
        let result = ping("not-an-address", 1000, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_ping_connection_refused() {
        let result = ping("127.0.0.1:9", 100, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_ping_localhost() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::scope(|s| {
            s.spawn(|| {
                let _ = listener.accept();
            });

            let result = ping(&format!("127.0.0.1:{}", port), 100, "ping");
            assert!(result.is_ok());
            assert!(result.unwrap().latency_ms.is_some());
        });
    }
}
