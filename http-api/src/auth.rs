use std::convert::TryFrom;

use chrono::{DateTime, Utc};
use ethers_core::types::{Signature, H160};
use serde::{Deserialize, Serialize};

use crate::error::Error;

#[derive(Deserialize, Serialize)]
pub struct AuthRequest {
    pub message: String,
    pub signature: Signature,
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
