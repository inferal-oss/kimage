//! Small static fixture executable for real sandbox/mount/cache acceptance.
//! Successful RUNs emit no input or secret bytes.
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") => {
            assert_eq!(fs::read("/bound/message").unwrap().len(), 32);
            assert_eq!(
                fs::metadata("/bound/probe").unwrap().permissions().mode() & 0o777,
                0o555
            );
            assert_eq!(fs::read("/run/secrets/token").unwrap().len(), 32);
            assert!(fs::write("/bound/mutation", b"denied").is_err());
            fs::write("/run-proof", b"bind-and-secret-mounts-checked").unwrap();
        }
        Some("wait") => {
            let seconds: u64 = args[2].parse().unwrap();
            assert!(seconds <= 300);
            std::thread::sleep(Duration::from_secs(seconds));
            fs::write("/run-proof", b"bounded-wait-complete").unwrap();
        }
        Some("diagnose") => {
            println!("kimage-diagnostic-start");
            std::thread::sleep(Duration::from_secs(8));
            for line in 0..5000 {
                println!("kimage-diagnostic-output-{line:04}");
            }
            eprintln!("kimage-diagnostic-failure-end");
            std::process::exit(23);
        }
        Some("stage") => {
            fs::create_dir_all("/destination/subdir").unwrap();
            fs::copy("/fixture/probe", "/destination/subdir/probe").unwrap();
            fs::set_permissions(
                "/destination/subdir/probe",
                fs::Permissions::from_mode(0o555),
            )
            .unwrap();
            fs::write("/destination/subdir/message", args[2].as_bytes()).unwrap();
            if let Ok(recipe) = fs::read("/fixture/recipe") {
                assert!(recipe.len() <= 65536);
                fs::write("/destination/subdir/Containerfile", recipe).unwrap();
            }
            if let Some(seconds) = args.get(3) {
                let seconds: u64 = seconds.parse().unwrap();
                assert!(seconds <= 300);
                std::thread::sleep(Duration::from_secs(seconds));
            }
        }
        _ => {
            println!("kimage build fixture ready");
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
    }
}
