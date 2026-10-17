//! Native test toolchain/runtime. No shell, network access or external dependencies.
use std::{env, fs, path::Path, thread, time::Duration};
fn main() {
    let args: Vec<_> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("transform") => {
            let text = fs::read_to_string(&args[2]).unwrap();
            fs::create_dir_all(Path::new(&args[3]).parent().unwrap()).unwrap();
            fs::write(&args[3], text.to_uppercase()).unwrap();
            println!("native toolchain executed on {}", env::consts::ARCH);
        }
        Some("serve") => {
            let text = fs::read_to_string(&args[2]).unwrap();
            println!("{}: {}", env::consts::ARCH, text.trim());
            loop { thread::park_timeout(Duration::from_secs(3600)); }
        }
        _ => panic!("unsupported fixture command"),
    }
}
