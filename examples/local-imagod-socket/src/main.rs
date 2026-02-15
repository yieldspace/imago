use std::{
    env, io,
    net::{SocketAddr, UdpSocket},
};

use tokio::{
    runtime::Builder,
    time::{Duration, sleep},
};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:5000";
const DEFAULT_PACKET_BUFFER_BYTES: usize = 65_507;
const IDLE_SLEEP_MILLIS: u64 = 5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Builder::new_current_thread().enable_time().build()?;
    runtime.block_on(run_udp_echo())?;
    Ok(())
}

async fn run_udp_echo() -> io::Result<()> {
    let bind_addr_text =
        env::var("IMAGO_SOCKET_BIND").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string());
    let bind_addr = bind_addr_text.parse::<SocketAddr>().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid IMAGO_SOCKET_BIND '{bind_addr_text}': {err}"),
        )
    })?;

    let socket = UdpSocket::bind(bind_addr)?;
    socket.set_nonblocking(true)?;
    println!("local-imagod-socket-app listening on udp://{bind_addr}");

    let mut buffer = vec![0_u8; DEFAULT_PACKET_BUFFER_BYTES];
    loop {
        let mut received_any = false;
        loop {
            match socket.recv_from(&mut buffer) {
                Ok((packet_len, peer)) => {
                    received_any = true;
                    let payload = &buffer[..packet_len];
                    if let Err(err) = socket.send_to(payload, peer) {
                        eprintln!("udp echo failed peer={peer}: {err}");
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err),
            }
        }

        if received_any {
            tokio::task::yield_now().await;
        } else {
            sleep(Duration::from_millis(IDLE_SLEEP_MILLIS)).await;
        }
    }
}
