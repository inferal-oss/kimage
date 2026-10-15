fn main() {
    println!("Hello from kimage!");
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(3600));
    }
}
