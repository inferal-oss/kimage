fn main() {
    if std::env::args().nth(1).as_deref() == Some("--read-loop-fixture") {
        use std::io::{Read, Write};
        let path = std::env::args().nth(2).expect("fixture digest path");
        assert!(path.starts_with("/v2/") && !path.contains(['\r', '\n']));
        let duration: u64 = std::env::args()
            .nth(3)
            .unwrap_or_else(|| "180".into())
            .parse()
            .unwrap();
        assert!(duration <= 600);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(duration);
        let (mut successes, mut failures) = (0_u64, 0_u64);
        while std::time::Instant::now() < end {
            let outcome = (|| -> std::io::Result<bool> {
                let mut stream = std::net::TcpStream::connect_timeout(
                    &"127.0.0.1:17641".parse().unwrap(),
                    std::time::Duration::from_secs(2),
                )?;
                stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
                stream.write_all(
                    format!(
                        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:17641\r\nAccept: application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )?;
                let mut bytes = Vec::new();
                stream.take(16384).read_to_end(&mut bytes)?;
                Ok(bytes.starts_with(b"HTTP/1.1 200"))
            })();
            if matches!(outcome, Ok(true)) {
                successes += 1;
            } else {
                failures += 1;
            }
            if (successes + failures) % 20 == 0 {
                println!("read-loop successes={successes} failures={failures}");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!("read-loop final successes={successes} failures={failures}");
        return;
    }

    if std::env::args().nth(1).as_deref() == Some("--dns-fixture") {
        use std::net::ToSocketAddrs;
        let host = std::env::args().nth(2).expect("fixture DNS authority");
        println!(
            "resolver: {}",
            std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default()
        );
        match host.to_socket_addrs() {
            Ok(addresses) => {
                for address in addresses {
                    println!("resolved: {address}");
                    println!(
                        "TCP: {:?}",
                        std::net::TcpStream::connect_timeout(
                            &address,
                            std::time::Duration::from_secs(3)
                        )
                        .map(|_| ())
                    );
                }
            }
            Err(error) => println!("DNS: {error}"),
        }
        return;
    }

    if std::env::args().nth(1).as_deref() == Some("--stage-fixture") {
        use std::io::{Read, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open("/source/input.oci.tar")
            .expect("create fixture archive");
        let count = std::io::copy(
            &mut std::io::stdin().take(1024 * 1024 * 1024 + 1),
            &mut file,
        )
        .expect("write fixture archive");
        assert!(count <= 1024 * 1024 * 1024, "fixture archive limit");
        file.flush().expect("flush fixture archive");
        file.sync_all().expect("persist fixture archive");
        return;
    }

    println!(
        "{}",
        std::env::var("KIMAGE_PROBE_MESSAGE").unwrap_or_else(|_| "kimage normal pull".into())
    );
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(3600));
    }
}
