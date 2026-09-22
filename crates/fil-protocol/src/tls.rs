//! Certificate pinning for the self-hosted QUIC transport. Obtain the public
//! certificate through the authenticated HTTPS control plane before connecting.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, SignatureScheme};

#[derive(Debug)]
pub struct PinnedServerCertificate(pub Vec<u8>);

impl ServerCertVerifier for PinnedServerCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if !self.0.is_empty() && end_entity.as_ref() == self.0.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General(
                "QUIC certificate does not match HTTPS pin".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_wrong_or_empty_certificate_pin() {
        let name = ServerName::try_from("localhost").unwrap();
        let cert = CertificateDer::from(vec![1, 2, 3]);
        for pin in [vec![], vec![9]] {
            assert!(
                PinnedServerCertificate(pin)
                    .verify_server_cert(
                        &cert,
                        &[],
                        &name,
                        &[],
                        UnixTime::since_unix_epoch(std::time::Duration::ZERO)
                    )
                    .is_err()
            );
        }
        assert!(
            PinnedServerCertificate(vec![1, 2, 3])
                .verify_server_cert(
                    &cert,
                    &[],
                    &name,
                    &[],
                    UnixTime::since_unix_epoch(std::time::Duration::ZERO)
                )
                .is_ok()
        );
    }
}
