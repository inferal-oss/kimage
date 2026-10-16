//! Cross-compiled test input. BuildKit and kubelet must execute the native ELF.
use std::{fs, time::Duration};
fn main() {
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => panic!("unsupported fixture architecture: {other}"),
    };
    let platform = format!("linux/{architecture}");
    if std::env::args().nth(1).as_deref() == Some("build") {
        fs::write("/built-platform", &platform).unwrap();
        return;
    }
    assert_eq!(fs::read_to_string("/built-platform").unwrap(), platform);
    println!("{platform}");
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}
