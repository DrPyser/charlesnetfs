use std::io::{Read, Write};
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
        Ok(mut stream) => {
            stream
                .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
                .ok();

            let write_result = stream.write_all(payload.as_bytes());

            if let Err(e) = write_result {
                return Err(
                    format!("write failed: {}", e)
                );
            }

            let mut buf = vec![0u8; 1];
            let read_result = stream.read_to_end(&mut buf);
            if let Err(read_err) = read_result {
                return Err(format!("read failed: {}", read_err))
            }
            let elapsed = start.elapsed().as_millis() as u64;
            // TODO: verify read_result == payload

            Ok(PingResult {
                latency_ms: Some(elapsed),
                payload: payload.to_string(),
            })
        }
        Err(e) => Err(format!("{}", e))
    }
}
