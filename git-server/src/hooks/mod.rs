pub mod post_receive;
pub mod pre_receive;
pub mod storage;
pub mod types;

use crate::error::Error;

/// Trait for shared default methods for accessing
/// GPG signed push certificate detail information, such as
/// signer name and email set in the `$GIT_PUSH_CERT_SIGNER` env.
///
/// e.g. `First Last <first.last@email.com>`
pub trait CertSignerDetails {
    /// returns the name of the GPG signer set from the `$GIT_PUSH_CERT_SIGNER` env.
    fn signer_name(cert_signer: Option<String>) -> Result<String, Error> {
        if let Some(signer) = cert_signer {
            let end = signer.find('<').unwrap_or_else(|| signer.len()) - 1;

            return Ok(signer[0..end].to_owned());
        }

        Err(Error::MissingCertificateSignerCredentials(
            "name".to_string(),
        ))
    }

    /// returns the email of the GPG signer set from the `$GIT_PUSH_CERT_SIGNER` env.
    fn signer_email(cert_signer: Option<String>) -> Result<String, Error> {
        if let Some(signer) = cert_signer {
            if let Some(start) = signer.find('<') {
                if let Some(end) = signer.find('>') {
                    return Ok(signer[start + 1..end].to_owned());
                }
            }
        }

        Err(Error::MissingCertificateSignerCredentials(
            "email".to_string(),
        ))
    }
}
