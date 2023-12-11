use std::sync::Arc;
use std::os::fd::AsRawFd;
use tokio_tun::Tun;

fn main() {
    test();
}

#[tokio::main]
async fn test() {
    let tun = Arc::new(
        Tun::builder()
            .name("test")            // if name is empty, then it is set by kernel.
            .tap(false)          // false (default): TUN, true: TAP.
            .packet_info(false)  // false: IFF_NO_PI, default is true.
            .up()                // or set it up manually using `sudo ip link set <tun-name> up`.
            .try_build()         // or `.try_build_mq(queues)` for multi-queue support.
            .unwrap(),
    );

    println!("tun created, name: {}, fd: {}", tun.name(), tun.as_raw_fd());

    // let (mut reader, mut _writer) = tokio::io::split(tun);

    // // Writer: simply clone Arced Tun.
    // let tun_c = tun.clone();
    // tokio::spawn(async move{
    //     let buf = b"data to be written";
    //     tun_c.send_all(buf).await.unwrap();
    // });

    // Reader
    let mut buf = [0u8; 1024];
    loop {
        let n = tun.recv(&mut buf).await.unwrap();
        println!("reading {} bytes: {:?}", n, &buf[..n]);
    }
}