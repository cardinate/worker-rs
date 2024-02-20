use anyhow::Result;
use tracing_subscriber::EnvFilter;
use std::path::PathBuf;

pub fn tests_data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

pub fn setup_tracing() -> Result<()> {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    Ok(())
}
