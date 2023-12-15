use std::net::SocketAddr;

use bytes::BytesMut;
use serde_derive::{Serialize, Deserialize};
use tokio::net::UdpSocket;
use validator::Validate;

use crate::core::GenericResult;
use crate::transport::Transport;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UdpTransportConfig {
    #[validate(length(min = 1))]
    bind_address: String,
    #[validate(length(min = 1))]
    peer_address: String,
}

pub struct UdpTransport {
    socket: UdpSocket,
    // last_peer: Option<SocketAddr>,
}

impl UdpTransport {
    pub async fn new(address: &str) -> GenericResult<Box<dyn Transport>> {
        let socket = UdpSocket::bind(address).await?;

    //     let mut buf = [0; 1024];
    // loop {
    // let (len, addr) = socket.recv_from(&mut buf).await?;
    // println!("{:?} bytes received from {:?}", len, addr);

    // let len = sock.send_to(&buf[..len], addr).await?;
    // println!("{:?} bytes sent", len);
    // }

        Ok(Box::new(UdpTransport {socket}))
    }

    // async fn handle(&self) {
    //     let mut buf = BytesMut::zeroed(1500 - 20 - 8); // MTU - IPv4 - UDP
    // }
}

impl Transport for UdpTransport {

}

    // # Example: one to many (bind)
    //
    // Using `bind` we can create a simple echo server that sends and recv's with many different clients:
    // ```no_run
    // use tokio::net::UdpSocket;
    // use std::io;
    //
    // #[tokio::main]
    // async fn main() -> io::Result<()> {
    //     }
    // }
    // ```
    //
    // # Example: one to one (connect)
    //
    // Or using `connect` we can echo with a single remote address using `send` and `recv`:
    // ```no_run
    // use tokio::net::UdpSocket;
    // use std::io;
    //
    // #[tokio::main]
    // async fn main() -> io::Result<()> {
    //     let sock = UdpSocket::bind("0.0.0.0:8080").await?;
    //
    //     let remote_addr = "127.0.0.1:59611";
    //     sock.connect(remote_addr).await?;
    //     let mut buf = [0; 1024];
    //     loop {
    //         let len = sock.recv(&mut buf).await?;
    //         println!("{:?} bytes received from {:?}", len, remote_addr);
    //
    //         let len = sock.send(&buf[..len]).await?;
    //         println!("{:?} bytes sent", len);
    //     }
    // }
    // ```
    //
    // # Example: Splitting with `Arc`
    //
    // Because `send_to` and `recv_from` take `&self`. It's perfectly alright
    // to use an `Arc<UdpSocket>` and share the references to multiple tasks.
    // Here is a similar "echo" example that supports concurrent
    // sending/receiving:
    //
    // ```no_run
    // use tokio::{net::UdpSocket, sync::mpsc};
    // use std::{io, net::SocketAddr, sync::Arc};
    //
    // #[tokio::main]
    // async fn main() -> io::Result<()> {
    //     let sock = UdpSocket::bind("0.0.0.0:8080".parse::<SocketAddr>().unwrap()).await?;
    //     let r = Arc::new(sock);
    //     let s = r.clone();
    //     let (tx, mut rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(1_000);
    //
    //     tokio::spawn(async move {
    //         while let Some((bytes, addr)) = rx.recv().await {
    //             let len = s.send_to(&bytes, &addr).await.unwrap();
    //             println!("{:?} bytes sent", len);
    //         }
    //     });
    //
    //     let mut buf = [0; 1024];
    //     loop {
    //         let (len, addr) = r.recv_from(&mut buf).await?;
    //         println!("{:?} bytes received from {:?}", len, addr);
    //         tx.send((buf[..len].to_vec(), addr)).await.unwrap();
    //     }
    // }
    // ```
    //
    //

// #[tokio::main]
// async fn main() {
//     let listener = TcpListener::bind("127.0.0.1:6379").await.unwrap();

//     loop {
//         let (socket, _) = listener.accept().await.unwrap();
//         // A new task is spawned for each inbound socket. The socket is
//         // moved to the new task and processed there.
//         tokio::spawn(async move {
//             process(socket).await;
//         });
//     }
// }

// #[tokio::main]
// async fn main() {
//     let handle = tokio::spawn(async {
//         // Do some async work
//         "return value"
//     });

//     // Do some other work

//     let out = handle.await.unwrap();
//     println!("GOT {}", out);
// }

// #[tokio::main]
// async fn main() {
//     // Bind the listener to the address
//     let listener = TcpListener::bind("127.0.0.1:6379").await.unwrap();

//     loop {
//         // The second item contains the IP and port of the new connection.
//         let (socket, _) = listener.accept().await.unwrap();
//         process(socket).await;
//     }
// }

// async fn process(socket: TcpStream) {
//     // The `Connection` lets us read/write redis **frames** instead of
//     // byte streams. The `Connection` type is defined by mini-redis.
//     let mut connection = Connection::new(socket);

//     if let Some(frame) = connection.read_frame().await.unwrap() {
//         println!("GOT: {:?}", frame);

//         // Respond with an error
//         let response = Frame::Error("unimplemented".to_string());
//         connection.write_frame(&response).await.unwrap();
//     }
// }