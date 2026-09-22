use slipstream_processing::{Executor, protocol::Config, serve};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 || arguments[0] != "--config" || !Path::new(&arguments[1]).is_absolute()
    {
        eprintln!("Usage: slipstream-processing-launcher --config /absolute/config.json");
        std::process::exit(2);
    }
    let result = (|| {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&arguments[1])
            .map_err(|_| ())?;
        let metadata = file.metadata().map_err(|_| ())?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o022 != 0
        {
            return Err(());
        }
        let mut bytes = Vec::new();
        file.take(16385).read_to_end(&mut bytes).map_err(|_| ())?;
        let executor = match Config::parse(&bytes) {
            Ok(config) => Executor::open(config),
            Err(_) => {
                slipstream_processing::film::Config::parse(&bytes).and_then(Executor::open_film)
            }
        }
        .map_err(|_| ())?;
        serve(executor).map_err(|_| ())
    })();
    if result.is_err() {
        eprintln!(
            "Processing qualification launcher unavailable; no image processing capability was enabled"
        );
        std::process::exit(1);
    }
}
