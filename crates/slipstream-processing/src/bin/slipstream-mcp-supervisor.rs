fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match slipstream_processing::run_supervisor(&arguments) {
        Ok(status) => std::process::exit(status),
        Err(error) => {
            eprintln!("slipstream-mcp-supervisor: {error}");
            std::process::exit(1);
        }
    }
}
