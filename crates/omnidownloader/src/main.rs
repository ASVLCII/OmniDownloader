#[tokio::main]
async fn main() {
    // Rust duplicates the stdio explicitly requested for each child. Prevent an
    // inherited CLI capture pipe from also leaking into the detached worker,
    // which would keep `subprocess.run(..., capture_output=True)` waiting for EOF.
    #[cfg(windows)]
    protect_standard_handles();
    if let Err(error) = omnidownloader::app::run().await {
        let message = omnidownloader::engine::redact(&format!("{error:#}"));
        if std::env::args().any(|arg| arg == "--json") {
            println!("{}", serde_json::json!({"type":"error","message":message}));
        }
        eprintln!("{message}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn protect_standard_handles() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT,
    };
    for handle in [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ] {
        let mut flags = 0;
        // Console pseudo-handles may not support handle-information APIs.
        unsafe {
            if GetHandleInformation(handle, &mut flags) != 0 && flags & HANDLE_FLAG_INHERIT != 0 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}
