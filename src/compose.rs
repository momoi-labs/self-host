use std::path::PathBuf;

/// Where the Platform keeps everything it generates: the DNS configuration, the
/// configuration, the certificates and each Application's data.
///
/// Under `cfg(test)` this is a temporary directory instead. Bootstrap writes
/// these files as a side effect of being called at all, so a unit test that
/// renders a Corefile with a fixture Host IP used to overwrite the DNS of a
/// Platform running on the same machine — `cargo test` broke name resolution
/// on the LAN. Tests get their own directory so that cannot happen.
pub fn platform_config_dir() -> PathBuf {
    #[cfg(test)]
    {
        test_config_dir()
    }
    #[cfg(not(test))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("self-host")
    }
}

#[cfg(test)]
fn test_config_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("self-host-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the test configuration directory");
        dir
    })
    .clone()
}

#[cfg(test)]
mod config_dir_tests {
    use super::platform_config_dir;

    /// A unit test once rewrote a running Platform's Corefile with a fixture
    /// Host IP, and name resolution on the LAN stopped working until someone
    /// noticed. Nothing the suite writes may land in the Operator's home.
    #[test]
    fn tests_never_write_to_the_operators_configuration() {
        let operators = dirs::home_dir()
            .expect("a home directory")
            .join(".config")
            .join("self-host");

        assert_ne!(platform_config_dir(), operators);
    }
}

#[cfg(test)]
mod tests {}
