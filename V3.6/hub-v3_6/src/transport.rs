use std::{collections::HashMap, time::Duration};

use reqwest::{StatusCode, blocking::Client};
use serde::Deserialize;
use serde_json::json;
use xhub_protocol_v3_6::{CanonicalEncode, RecoveryPackage};

use crate::api::{DeliveryTransportError, PROTOCOL_VERSION_HEADER, RecoveryPackageTransport};

#[derive(Debug, Clone)]
pub struct WatchtowerHttpTransport {
    endpoint: String,
    bearer_token: String,
    expected_recipient_id: String,
    timeout: Duration,
}

#[derive(Debug, Deserialize)]
struct PackageResponse {
    recovery_package_content_hash: String,
}

impl WatchtowerHttpTransport {
    pub fn new(
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
        expected_recipient_id: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, DeliveryTransportError> {
        let base_url = base_url.into();
        let bearer_token = bearer_token.into();
        let expected_recipient_id = expected_recipient_id.into();
        if bearer_token.is_empty() || expected_recipient_id.is_empty() {
            return Err(final_error(
                "watchtower token and recipient ID are required",
            ));
        }
        Ok(Self {
            endpoint: format!(
                "{}/api/v3.6/recovery-packages",
                base_url.trim_end_matches('/')
            ),
            bearer_token,
            expected_recipient_id,
            timeout,
        })
    }
}

impl RecoveryPackageTransport for WatchtowerHttpTransport {
    fn recipient_ids(&self) -> Vec<String> {
        vec![self.expected_recipient_id.clone()]
    }

    fn deliver(
        &self,
        recipient_id: &str,
        recipient_kind: &str,
        idempotency_key: &str,
        package: &RecoveryPackage,
    ) -> Result<(), DeliveryTransportError> {
        if recipient_kind != "WATCHTOWER" || recipient_id != self.expected_recipient_id {
            return Err(final_error(
                "delivery recipient does not match this transport",
            ));
        }
        let expected_hash = package
            .content_hash()
            .map_err(|error| final_error(error.to_string()))?;
        let client = Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|error| final_error(error.to_string()))?;
        let response = client
            .post(&self.endpoint)
            .bearer_auth(&self.bearer_token)
            .header(PROTOCOL_VERSION_HEADER, "0x0360")
            .header("x-idempotency-key", idempotency_key)
            .json(&json!({
                "protocol_version": "0x0360",
                "recovery_package_canonical_hex": hex::encode(package.canonical_bytes())
            }))
            .send()
            .map_err(|error| DeliveryTransportError {
                retryable: true,
                message: error.to_string(),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(DeliveryTransportError {
                retryable: status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
                message: format!("watchtower returned HTTP {status}"),
            });
        }
        let body: PackageResponse = response.json().map_err(|error| DeliveryTransportError {
            retryable: true,
            message: format!("invalid watchtower response: {error}"),
        })?;
        if body.recovery_package_content_hash != hex::encode(expected_hash) {
            return Err(final_error("watchtower response content hash mismatch"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct WatchtowerHttpTransportSet {
    transports: HashMap<String, WatchtowerHttpTransport>,
}

impl WatchtowerHttpTransportSet {
    pub fn new(transports: Vec<WatchtowerHttpTransport>) -> Result<Self, DeliveryTransportError> {
        if transports.len() != 3 {
            return Err(final_error(
                "exactly three Watchtower transports are required",
            ));
        }
        let mut by_recipient = HashMap::with_capacity(transports.len());
        for transport in transports {
            let recipient_id = transport.expected_recipient_id.clone();
            if by_recipient.insert(recipient_id, transport).is_some() {
                return Err(final_error("Watchtower recipient IDs must be distinct"));
            }
        }
        Ok(Self {
            transports: by_recipient,
        })
    }
}

impl RecoveryPackageTransport for WatchtowerHttpTransportSet {
    fn recipient_ids(&self) -> Vec<String> {
        let mut recipients = self.transports.keys().cloned().collect::<Vec<_>>();
        recipients.sort();
        recipients
    }

    fn deliver(
        &self,
        recipient_id: &str,
        recipient_kind: &str,
        idempotency_key: &str,
        package: &RecoveryPackage,
    ) -> Result<(), DeliveryTransportError> {
        let transport = self
            .transports
            .get(recipient_id)
            .ok_or_else(|| final_error("unknown Watchtower recipient ID"))?;
        transport.deliver(recipient_id, recipient_kind, idempotency_key, package)
    }
}

fn final_error(message: impl Into<String>) -> DeliveryTransportError {
    DeliveryTransportError {
        retryable: false,
        message: message.into(),
    }
}
