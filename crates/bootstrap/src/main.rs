//! Cached carrier executes this static launcher; no shell or carrier tools used.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::Command,
};
fn main() {
    if let Err(error) = run() {
        eprintln!("kimage-bootstrap: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 6 {
        return Err("usage: kimage-bootstrap CHUNKS COUNT SHA256 OUTPUT COMMAND_ARGS...".into());
    }
    let count: usize = args[2].parse()?;
    if !(1..=256).contains(&count)
        || args[3].len() != 64
        || !args[3]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid inventory".into());
    }
    let dest = Path::new(&args[4]);
    let pending = dest.with_extension("partial");
    let compressed = dest.with_extension("partial.gz");
    let mut output = File::create(&compressed)?;
    let mut hash = Sha256::new();
    for i in 0..count {
        let mut input = File::open(Path::new(&args[1]).join(format!("{i:04}")))?;
        let size = input.metadata()?.len();
        if size == 0 || size > 524288 {
            return Err("invalid chunk size".into());
        }
        let mut buffer = [0; 65536];
        let mut read = 0;
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            read += n;
            if read > 524288 {
                return Err("chunk grew beyond bound".into());
            }
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
        }
    }
    if hex::encode(hash.finalize()) != args[3] {
        fs::remove_file(&compressed)?;
        return Err("payload digest mismatch".into());
    }
    output.sync_all()?;
    drop(output);
    let input = flate2::read::GzDecoder::new(File::open(&compressed)?);
    let mut limited = input.take(134217729);
    let mut output = File::create(&pending)?;
    let bytes = std::io::copy(&mut limited, &mut output)?;
    if bytes > 134217728 {
        return Err("executable too large".into());
    }
    output.sync_all()?;
    drop(output);
    fs::remove_file(&compressed)?;
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o555))?;
    fs::rename(&pending, dest)?;
    Err(Command::new(dest).args(&args[5..]).exec().into())
}
