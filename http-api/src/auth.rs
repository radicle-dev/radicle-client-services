use std::convert::TryFrom;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use ethers_core::types::{Signature, H160};
use serde::{Deserialize, Serialize};

use crate::error::Error;

#[derive(Deserialize, Serialize)]
pub struct AuthRequest {
    pub message: String,
    #[serde(deserialize_with = "deserialize_signature")]
    pub signature: Signature,
}

fn deserialize_signature<'de, D>(deserializer: D) -> Result<Signature, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let buf = String::deserialize(deserializer)?;
    Signature::from_str(&buf).map_err(serde::de::Error::custom)
}

pub enum AuthState {
    Authorized(Session),
    Unauthorized {
        nonce: String,
        expiration_time: DateTime<Utc>,
    },
}

// We copy the implementation of siwe::Message here to derive Serialization and Debug
#[derive(Clone, Serialize)]
pub struct Session {
    pub domain: String,
    pub address: H160,
    pub statement: String,
    pub uri: String,
    pub version: u64,
    pub chain_id: u64,
    pub nonce: String,
    pub issued_at: DateTime<Utc>,
    pub expiration_time: Option<DateTime<Utc>>,
    pub resources: Vec<String>,
}

impl TryFrom<siwe::Message> for Session {
    type Error = Error;

    fn try_from(message: siwe::Message) -> Result<Session, Error> {
        let statement = message.statement.ok_or(Error::Auth("No statement found"))?;

        Ok(Session {
            domain: message.domain.host().to_string(),
            address: H160(message.address),
            statement,
            uri: message.uri.to_string(),
            version: message.version as u64,
            chain_id: message.chain_id,
            nonce: message.nonce,
            issued_at: message.issued_at.as_ref().with_timezone(&Utc),
            expiration_time: message
                .expiration_time
                .map(|x| x.as_ref().with_timezone(&Utc)),
            resources: message.resources.iter().map(|r| r.to_string()).collect(),
        })
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_auth_request_de() {
        let json = serde_json::json!({
            "message": "Hello World!",
            "signature": "20096c6ed2bcccb88c9cafbbbbda7a5a3cff6d0ca318c07faa58464083ca40a92f899fbeb26a4c763a7004b13fd0f1ba6c321d4e3a023e30f63c40d4154b99a41c"
        });

        let _req: super::AuthRequest = serde_json::from_value(json).unwrap();
    }
}
