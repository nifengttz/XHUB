use std::collections::HashMap;

use anyhow::{Result, bail};
use snow::{Builder, HandshakeState, TransportState, params::NoiseParams};

const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_SHA256";
const MAX_FRAME_BYTES: usize = 64 * 1024;

enum SessionState {
    Handshake(Box<HandshakeState>),
    Transport(TransportState),
}

/// In-memory Hub endpoint for the wallet's ephemeral Noise XX session.
/// Session keys intentionally never enter SQLite, logs, or browser responses.
pub struct NoiseHubSessions {
    local_private_key: [u8; 32],
    sessions: HashMap<String, SessionState>,
}

impl NoiseHubSessions {
    pub fn new(local_private_key: [u8; 32]) -> Self {
        Self {
            local_private_key,
            sessions: HashMap::new(),
        }
    }

    /// Processes the next handshake frame. The caller supplies a request-bound
    /// session id; a different request must never reuse that id.
    pub fn handshake_frame(&mut self, session_id: &str, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        if frame.len() > MAX_FRAME_BYTES {
            bail!("Noise frame is too large");
        }
        if !self.sessions.contains_key(session_id) {
            let params: NoiseParams = NOISE_PARAMS.parse()?;
            let responder = Builder::new(params)
                .local_private_key(&self.local_private_key)
                .build_responder()?;
            self.sessions.insert(
                session_id.to_owned(),
                SessionState::Handshake(Box::new(responder)),
            );
        }
        let state = self.sessions.get_mut(session_id).expect("inserted above");
        let SessionState::Handshake(handshake) = state else {
            bail!("Noise handshake is already complete");
        };
        let mut received = vec![0; MAX_FRAME_BYTES];
        handshake.read_message(frame, &mut received)?;
        if handshake.is_handshake_finished() {
            let state = self.sessions.remove(session_id).expect("present");
            let SessionState::Handshake(handshake) = state else {
                unreachable!()
            };
            self.sessions.insert(
                session_id.to_owned(),
                SessionState::Transport((*handshake).into_transport_mode()?),
            );
            return Ok(Some(Vec::new()));
        }
        let mut response = vec![0; MAX_FRAME_BYTES];
        let written = handshake.write_message(&[], &mut response)?;
        response.truncate(written);
        Ok(Some(response))
    }

    pub fn receive(&mut self, session_id: &str, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() > MAX_FRAME_BYTES {
            bail!("Noise frame is too large");
        }
        let Some(SessionState::Transport(transport)) = self.sessions.get_mut(session_id) else {
            bail!("Noise session is not established");
        };
        let mut plaintext = vec![0; MAX_FRAME_BYTES];
        let written = transport.read_message(ciphertext, &mut plaintext)?;
        plaintext.truncate(written);
        Ok(plaintext)
    }

    pub fn send(&mut self, session_id: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > MAX_FRAME_BYTES {
            bail!("Noise message is too large");
        }
        let Some(SessionState::Transport(transport)) = self.sessions.get_mut(session_id) else {
            bail!("Noise session is not established");
        };
        let mut ciphertext = vec![0; plaintext.len() + 32];
        let written = transport.write_message(plaintext, &mut ciphertext)?;
        ciphertext.truncate(written);
        Ok(ciphertext)
    }

    pub fn remove(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xx_handshake_encrypts_bidirectional_messages() {
        let params: NoiseParams = NOISE_PARAMS.parse().unwrap();
        let builder = Builder::new(params);
        let keypair = builder.generate_keypair().unwrap();
        let mut initiator = builder
            .local_private_key(&keypair.private)
            .build_initiator()
            .unwrap();
        let mut hub = NoiseHubSessions::new([7; 32]);
        let mut frame = vec![0; MAX_FRAME_BYTES];
        let size = initiator.write_message(&[], &mut frame).unwrap();
        let response = hub
            .handshake_frame("session-1", &frame[..size])
            .unwrap()
            .unwrap();
        initiator.read_message(&response, &mut frame).unwrap();
        let size = initiator.write_message(&[], &mut frame).unwrap();
        let response = hub
            .handshake_frame("session-1", &frame[..size])
            .unwrap()
            .unwrap();
        assert!(response.is_empty());
        let mut wallet = initiator.into_transport_mode().unwrap();
        let size = wallet.write_message(b"wallet_hello", &mut frame).unwrap();
        assert_eq!(
            hub.receive("session-1", &frame[..size]).unwrap(),
            b"wallet_hello"
        );
        let encrypted = hub.send("session-1", b"funding_request_final").unwrap();
        let size = wallet.read_message(&encrypted, &mut frame).unwrap();
        assert_eq!(&frame[..size], b"funding_request_final");
    }
}
