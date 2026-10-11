fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match darktable_edit::engine::run_supervisor(&arguments) {
        Ok(status) => std::process::exit(status),
        Err(error) => {
            eprintln!("darktable-edit-supervisor: {error}");
            std::process::exit(1);
        }
    }
}
