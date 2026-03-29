use std::net::TcpStream;
use std::time::{Duration, Instant};
use std::fmt::{Display, Formatter};

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
        Err(e) => Err(format!("{}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        //rpcbind very likely enabled
        let result = ping("127.0.0.1:111", 20, "ping");
        match result {
            Ok(r) => {
                assert!(r.latency_ms.is_some());
                assert_eq!(r.payload, "ping");
            }
            Err(e) => {
                panic!("{}", e);
                // assert!(e.contains("Connection refused") || e.contains("timed out"));
            }
        }
    }
}
