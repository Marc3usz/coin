use std::path::Path;

pub(super) fn file_bytes(prefix: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    [
        prefix.as_slice(),
        &(payload.len() as u64).to_be_bytes(),
        payload,
    ]
    .concat()
}

pub(super) fn write_file_with_prefix(
    path: &Path,
    prefix: &[u8; 4],
    payload: &[u8],
) -> anyhow::Result<()> {
    std::fs::write(path, file_bytes(prefix, payload))?;
    Ok(())
}

pub(super) fn verify_prefixed_file(path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(bytes.len() >= 12, "file too short: {}", path.display());
    anyhow::ensure!(
        &bytes[0..4] == b"CBLK" || &bytes[0..4] == b"CRCP",
        "bad magic: {}",
        path.display()
    );
    let len = u64::from_be_bytes(bytes[4..12].try_into().unwrap()) as usize;
    anyhow::ensure!(bytes.len() == 12 + len, "bad length: {}", path.display());
    Ok(())
}
