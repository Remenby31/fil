use snow::{Builder, HandshakeState, TransportState};
use std::sync::Mutex;

const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const MAX_MSG_LEN: usize = 65535;

#[derive(Debug)]
pub struct KeyPair {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

impl KeyPair {
    pub fn generate() -> Self {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap());
        let keypair = builder.generate_keypair().unwrap();
        Self {
            private: keypair.private.to_vec(),
            public: keypair.public.to_vec(),
        }
    }

    pub fn public_b64(&self) -> String {
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &self.public)
    }
}

pub struct NoiseInitiator {
    state: InitiatorState,
}

enum InitiatorState {
    Handshake(Box<HandshakeState>),
    Transitioning,
}

impl NoiseInitiator {
    pub fn new(local_key: &KeyPair) -> Self {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap())
            .local_private_key(&local_key.private)
            .build_initiator()
            .unwrap();

        Self {
            state: InitiatorState::Handshake(Box::new(builder)),
        }
    }

    pub fn handshake_write(&mut self) -> Vec<u8> {
        let InitiatorState::Handshake(ref mut hs) = self.state else {
            panic!("not in handshake state");
        };
        let mut buf = vec![0u8; MAX_MSG_LEN];
        let len = hs.write_message(&[], &mut buf).unwrap();
        buf.truncate(len);
        buf
    }

    pub fn handshake_read(&mut self, msg: &[u8]) -> Vec<u8> {
        let InitiatorState::Handshake(ref mut hs) = self.state else {
            panic!("not in handshake state");
        };
        let mut buf = vec![0u8; MAX_MSG_LEN];
        let len = hs.read_message(msg, &mut buf).unwrap();
        buf.truncate(len);
        buf
    }

    pub fn into_transport(mut self) -> NoiseTransport {
        let InitiatorState::Handshake(hs) = std::mem::replace(
            &mut self.state,
            InitiatorState::Transitioning,
        ) else {
            panic!("not in handshake state");
        };
        let transport = hs.into_transport_mode().unwrap();
        NoiseTransport {
            state: Mutex::new(transport),
        }
    }

    pub fn is_handshake_finished(&self) -> bool {
        match &self.state {
            InitiatorState::Handshake(hs) => hs.is_handshake_finished(),
            _ => false,
        }
    }
}

pub struct NoiseResponder {
    state: ResponderState,
}

enum ResponderState {
    Handshake(Box<HandshakeState>),
    Transitioning,
}

impl NoiseResponder {
    pub fn new(local_key: &KeyPair) -> Self {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap())
            .local_private_key(&local_key.private)
            .build_responder()
            .unwrap();

        Self {
            state: ResponderState::Handshake(Box::new(builder)),
        }
    }

    pub fn handshake_read(&mut self, msg: &[u8]) -> Vec<u8> {
        let ResponderState::Handshake(ref mut hs) = self.state else {
            panic!("not in handshake state");
        };
        let mut buf = vec![0u8; MAX_MSG_LEN];
        let len = hs.read_message(msg, &mut buf).unwrap();
        buf.truncate(len);
        buf
    }

    pub fn handshake_write(&mut self) -> Vec<u8> {
        let ResponderState::Handshake(ref mut hs) = self.state else {
            panic!("not in handshake state");
        };
        let mut buf = vec![0u8; MAX_MSG_LEN];
        let len = hs.write_message(&[], &mut buf).unwrap();
        buf.truncate(len);
        buf
    }

    pub fn into_transport(mut self) -> NoiseTransport {
        let ResponderState::Handshake(hs) = std::mem::replace(
            &mut self.state,
            ResponderState::Transitioning,
        ) else {
            panic!("not in handshake state");
        };
        let transport = hs.into_transport_mode().unwrap();
        NoiseTransport {
            state: Mutex::new(transport),
        }
    }

    pub fn is_handshake_finished(&self) -> bool {
        match &self.state {
            ResponderState::Handshake(hs) => hs.is_handshake_finished(),
            _ => false,
        }
    }
}

pub struct NoiseTransport {
    state: Mutex<TransportState>,
}

impl NoiseTransport {
    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let mut state = self.state.lock().unwrap();
        let mut buf = vec![0u8; plaintext.len() + 16]; // 16 bytes for AEAD tag
        let len = state.write_message(plaintext, &mut buf).unwrap();
        buf.truncate(len);
        buf
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, snow::Error> {
        let mut state = self.state.lock().unwrap();
        let mut buf = vec![0u8; ciphertext.len()];
        let len = state.read_message(ciphertext, &mut buf)?;
        buf.truncate(len);
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() {
        let kp = KeyPair::generate();
        assert_eq!(kp.public.len(), 32);
        assert_eq!(kp.private.len(), 32);
    }

    #[test]
    fn test_full_handshake_and_transport() {
        let initiator_key = KeyPair::generate();
        let responder_key = KeyPair::generate();

        let mut initiator = NoiseInitiator::new(&initiator_key);
        let mut responder = NoiseResponder::new(&responder_key);

        // XX handshake: 3 messages
        // 1. Initiator → Responder (e)
        let msg1 = initiator.handshake_write();
        responder.handshake_read(&msg1);

        // 2. Responder → Initiator (e, ee, s, es)
        let msg2 = responder.handshake_write();
        initiator.handshake_read(&msg2);

        // 3. Initiator → Responder (s, se)
        let msg3 = initiator.handshake_write();
        responder.handshake_read(&msg3);

        assert!(initiator.is_handshake_finished());
        assert!(responder.is_handshake_finished());

        // Convert to transport mode
        let i_transport = initiator.into_transport();
        let r_transport = responder.into_transport();

        // Test encryption/decryption
        let plaintext = b"hello from fil terminal";
        let ciphertext = i_transport.encrypt(plaintext);

        assert_ne!(&ciphertext, plaintext);
        assert!(ciphertext.len() > plaintext.len());

        let decrypted = r_transport.decrypt(&ciphertext).unwrap();
        assert_eq!(&decrypted, plaintext);

        // Test reverse direction
        let plaintext2 = b"response from phone";
        let ciphertext2 = r_transport.encrypt(plaintext2);
        let decrypted2 = i_transport.decrypt(&ciphertext2).unwrap();
        assert_eq!(&decrypted2, plaintext2);
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let ik = KeyPair::generate();
        let rk = KeyPair::generate();

        let mut initiator = NoiseInitiator::new(&ik);
        let mut responder = NoiseResponder::new(&rk);

        let msg1 = initiator.handshake_write();
        responder.handshake_read(&msg1);
        let msg2 = responder.handshake_write();
        initiator.handshake_read(&msg2);
        let msg3 = initiator.handshake_write();
        responder.handshake_read(&msg3);

        let i_transport = initiator.into_transport();
        let r_transport = responder.into_transport();

        let ciphertext = i_transport.encrypt(b"secret data");
        let mut tampered = ciphertext.clone();
        tampered[0] ^= 0xFF;

        assert!(r_transport.decrypt(&tampered).is_err());
    }
}
