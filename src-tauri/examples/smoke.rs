//! Explicit opt-in network check. Saves one real file; never opens its content.
use anyhow::{Context, Result};
use boton_lib::{download, network::SafeClient, sources};
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let destination = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("Pass a dedicated smoke-test download directory")?,
    );
    let client = SafeClient::new()?;
    let candidate =
        tokio::time::timeout(Duration::from_secs(90), sources::discover(&client)).await??;
    println!(
        "Discovered {} from {}",
        candidate.filename, candidate.source
    );
    let prepared = download::prepare(&client, candidate).await?;
    anyhow::ensure!(
        !download::requires_confirmation(prepared.total),
        "Smoke test refuses unknown or >100 MiB files; rerun to sample another"
    );
    let receipt =
        download::download(prepared, &destination, &CancellationToken::new(), |_, _| {}).await?;
    anyhow::ensure!(
        std::fs::metadata(&receipt.path)?.len() == receipt.bytes,
        "Saved length differs"
    );
    println!(
        "Saved {} bytes at {}\nSHA256 {}",
        receipt.bytes,
        receipt.path.display(),
        receipt.sha256
    );
    Ok(())
}
