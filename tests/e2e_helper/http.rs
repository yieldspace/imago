use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

pub fn wait_http_response(port: u16, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() > deadline {
            bail!("timed out waiting for HTTP response");
        }

        if let Ok(response) = http_get(port) {
            if parse_http_status(&response).is_some() {
                return Ok(response);
            }
        }

        thread::sleep(Duration::from_millis(200));
    }
}

pub fn http_get(port: u16) -> Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

pub fn parse_http_status(response: &str) -> Option<u16> {
    let line = response.lines().next()?;
    let mut parts = line.split_whitespace();
    let _http_version = parts.next()?;
    parts.next()?.parse().ok()
}
