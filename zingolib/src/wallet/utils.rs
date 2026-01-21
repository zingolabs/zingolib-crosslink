//! TODO: Add Mod Description Here!
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use zcash_primitives::transaction::TxId;
use zcash_protocol::memo::MemoBytes;

/// TODO: Add Doc Comment Here!
pub fn read_string<R: Read>(mut reader: R) -> io::Result<String> {
    // Strings are written as <littleendian> len + bytes
    let str_len = reader.read_u64::<LittleEndian>()?;
    let mut str_bytes = vec![0; str_len as usize];
    reader.read_exact(&mut str_bytes)?;

    let str = String::from_utf8(str_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    Ok(str)
}

/// TODO: Add Doc Comment Here!
pub fn write_string<W: Write>(mut writer: W, s: &String) -> io::Result<()> {
    // Strings are written as len + utf8
    writer.write_u64::<LittleEndian>(s.len() as u64)?;
    writer.write_all(s.as_bytes())
}

/// Interpret a string or hex-encoded memo, and return a Memo object
pub fn interpret_memo_string(memo_str: String) -> Result<MemoBytes, String> {
    // If the string starts with an "0x", and contains only hex chars ([a-f0-9]+) then
    // interpret it as a hex
    let s_bytes = if memo_str.to_lowercase().starts_with("0x") {
        match hex::decode(&memo_str[2..memo_str.len()]) {
            Ok(data) => data,
            Err(_) => Vec::from(memo_str.as_bytes()),
        }
    } else {
        Vec::from(memo_str.as_bytes())
    };

    MemoBytes::from_bytes(&s_bytes)
        .map_err(|_| format!("Error creating output. Memo '{memo_str:?}' is too long"))
}

/// TODO: Add Doc Comment Here!
#[must_use]
pub fn txid_from_slice(txid: &[u8]) -> TxId {
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(txid);
    TxId::from_bytes(txid_bytes)
}

/// Returns the downloaded Sapling parameters as bytes.
pub(crate) fn read_sapling_params() -> Result<(Vec<u8>, Vec<u8>), String> {
    use crate::SaplingParams;
    let mut sapling_output = vec![];
    sapling_output.extend_from_slice(
        SaplingParams::get("sapling-output.params")
            .unwrap()
            .data
            .as_ref(),
    );

    let mut sapling_spend = vec![];
    sapling_spend.extend_from_slice(
        SaplingParams::get("sapling-spend.params")
            .unwrap()
            .data
            .as_ref(),
    );
    Ok((sapling_output, sapling_spend))
}

fn ensure_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

pub fn ensure_sapling_params_on_disk(params_dir: PathBuf) -> Result<(PathBuf, PathBuf), String> {
    let (output_bytes, spend_bytes) = read_sapling_params()?;

    let output_path = params_dir.join("sapling-output.params");
    let spend_path = params_dir.join("sapling-spend.params");

    ensure_file(&output_path, &output_bytes)?;
    ensure_file(&spend_path, &spend_bytes)?;

    Ok((spend_path, output_path))
}
