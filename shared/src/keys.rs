use librad::PeerId;

/// Get the SSH key fingerprint from a peer id.
/// This is the output of `ssh-add -l`.
pub fn to_ssh_fingerprint(peer_id: &PeerId) -> Result<String, std::io::Error> {
    use byteorder::{BigEndian, WriteBytesExt};
    use sha2::Digest;

    let mut buf = Vec::new();
    let name = b"ssh-ed25519";
    let key = peer_id.as_public_key().as_ref();

    buf.write_u32::<BigEndian>(name.len() as u32)?;
    buf.extend_from_slice(name);
    buf.write_u32::<BigEndian>(key.len() as u32)?;
    buf.extend_from_slice(key);

    let sha = sha2::Sha256::digest(&buf).to_vec();
    let encoded = base64::encode(sha);

    Ok(format!("SHA256:{}", encoded.trim_end_matches('=')))
}
