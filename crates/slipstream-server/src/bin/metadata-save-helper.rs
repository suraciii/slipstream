//! Exclusive save session helper launched by the metadata supervisor. See
//! `slipstream_server::metadata_service` for the session contract.
fn main() {
    std::process::exit(slipstream_server::metadata_service::helper_entrypoint());
}
