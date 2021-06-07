use radicle_daemon::PeerId;
use radicle_keystore::sign::ed25519;

pub use ed25519::PublicKey;

#[derive(Clone, Debug)]
pub struct Signer {
    public_key: PublicKey,
}

impl Signer {
    pub fn new(peer_id: PeerId) -> Self {
        use std::convert::TryInto as _;

        let bytes = peer_id.as_public_key().as_ref();
        let public_key = PublicKey(bytes.to_owned().try_into().unwrap());

        Self { public_key }
    }
}

#[async_trait::async_trait]
impl ed25519::Signer for Signer {
    type Error = std::convert::Infallible;

    fn public_key(&self) -> ed25519::PublicKey {
        self.public_key
    }

    async fn sign(&self, _data: &[u8]) -> Result<ed25519::Signature, Self::Error> {
        unreachable!()
    }
}
