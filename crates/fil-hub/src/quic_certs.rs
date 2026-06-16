use anyhow::Result;
use std::path::Path;
use tracing::info;

pub struct QuicCerts {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl QuicCerts {
    pub fn load_or_generate(data_dir: &str) -> Result<Self> {
        let cert_path = Path::new(data_dir).join("quic-cert.der");
        let key_path = Path::new(data_dir).join("quic-key.der");

        if cert_path.exists() && key_path.exists() {
            let cert_der = std::fs::read(&cert_path)?;
            let key_der = std::fs::read(&key_path)?;
            info!("loaded existing QUIC certificates");
            return Ok(Self { cert_der, key_der });
        }

        let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
        let mut params = rcgen::CertificateParams::new(vec![
            "fil-hub".to_string(),
            "localhost".to_string(),
        ])?;
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            rcgen::DnValue::Utf8String("fil-hub".to_string()),
        );

        let cert = params.self_signed(&key_pair)?;

        let cert_der = cert.der().to_vec();
        let key_der = key_pair.serialize_der();

        std::fs::create_dir_all(data_dir)?;
        std::fs::write(&cert_path, &cert_der)?;
        std::fs::write(&key_path, &key_der)?;

        info!("generated new QUIC certificates");
        Ok(Self { cert_der, key_der })
    }

    pub fn fingerprint(&self) -> String {
        use std::fmt::Write;
        let hash = ring::digest::digest(&ring::digest::SHA256, &self.cert_der);
        let mut hex = String::new();
        for byte in hash.as_ref() {
            write!(hex, "{byte:02x}").ok();
        }
        hex
    }
}
