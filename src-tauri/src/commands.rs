//! Tauri commands exposed to the frontend.

/// The reply returned by [`ping`], used by the frontend to confirm the
/// backend is reachable.
#[must_use]
pub fn ping_reply() -> String {
    format!("satsuma {}", env!("CARGO_PKG_VERSION"))
}

/// Health check command: returns the backend name and version.
#[tauri::command]
pub fn ping() -> String {
    ping_reply()
}

#[cfg(test)]
mod tests {
    use super::ping_reply;

    #[test]
    fn ping_reply_contains_name_and_version() {
        let reply = ping_reply();
        assert!(reply.starts_with("satsuma "));
        assert!(reply.ends_with(env!("CARGO_PKG_VERSION")));
    }
}
