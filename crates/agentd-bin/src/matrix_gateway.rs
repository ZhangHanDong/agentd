//! Native authenticated Matrix/Robrix gateway composition and command normalization.

use std::fmt;
use std::sync::Arc;

use agentd_core::ports::{
    Clock, MatrixAttachmentRef, MatrixCommandClass, MatrixCommandReceipt,
    MatrixGatewayCommandRequest, MatrixGatewayDeliveryPort, MatrixGatewayError,
    MatrixGatewayIdentityPort, MatrixGatewayPort, MatrixTransportProvenance,
    NormalizedMatrixCommand,
};
use agentd_core::types::{ProjectExecutionSnapshotRef, ProjectRoomBindingRef};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixInboundAttachment {
    pub bytes: Vec<u8>,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixInboundTransportEvent {
    pub provenance: MatrixTransportProvenance,
    pub binding_ref: ProjectRoomBindingRef,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub body: String,
    pub attachments: Vec<MatrixInboundAttachment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixGatewayProviderKind {
    Identity,
    Gateway,
    Delivery,
    TrustedClock,
}

impl MatrixGatewayProviderKind {
    pub const ALL: [Self; 4] = [
        Self::Identity,
        Self::Gateway,
        Self::Delivery,
        Self::TrustedClock,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "matrix_identity",
            Self::Gateway => "matrix_gateway",
            Self::Delivery => "matrix_delivery",
            Self::TrustedClock => "trusted_clock",
        }
    }
}

impl fmt::Display for MatrixGatewayProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MatrixGatewayStartupError {
    #[error("enterprise Matrix gateway startup missing closed provider: {0}")]
    MissingProvider(MatrixGatewayProviderKind),
}

#[derive(Default)]
pub struct MatrixGatewayProviders {
    identity: Option<Arc<dyn MatrixGatewayIdentityPort>>,
    gateway: Option<Arc<dyn MatrixGatewayPort>>,
    delivery: Option<Arc<dyn MatrixGatewayDeliveryPort>>,
    trusted_clock: Option<Arc<dyn Clock>>,
}

impl fmt::Debug for MatrixGatewayProviders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MatrixGatewayProviders")
            .field("identity", &self.identity.is_some())
            .field("gateway", &self.gateway.is_some())
            .field("delivery", &self.delivery.is_some())
            .field("trusted_clock", &self.trusted_clock.is_some())
            .finish()
    }
}

impl MatrixGatewayProviders {
    #[must_use]
    pub fn new(
        identity: Arc<dyn MatrixGatewayIdentityPort>,
        gateway: Arc<dyn MatrixGatewayPort>,
        delivery: Arc<dyn MatrixGatewayDeliveryPort>,
        trusted_clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            identity: Some(identity),
            gateway: Some(gateway),
            delivery: Some(delivery),
            trusted_clock: Some(trusted_clock),
        }
    }

    #[must_use]
    pub fn without(mut self, provider: MatrixGatewayProviderKind) -> Self {
        match provider {
            MatrixGatewayProviderKind::Identity => self.identity = None,
            MatrixGatewayProviderKind::Gateway => self.gateway = None,
            MatrixGatewayProviderKind::Delivery => self.delivery = None,
            MatrixGatewayProviderKind::TrustedClock => self.trusted_clock = None,
        }
        self
    }

    fn has(&self, provider: MatrixGatewayProviderKind) -> bool {
        match provider {
            MatrixGatewayProviderKind::Identity => self.identity.is_some(),
            MatrixGatewayProviderKind::Gateway => self.gateway.is_some(),
            MatrixGatewayProviderKind::Delivery => self.delivery.is_some(),
            MatrixGatewayProviderKind::TrustedClock => self.trusted_clock.is_some(),
        }
    }
}

pub fn build_agentd_matrix_gateway(
    mut providers: MatrixGatewayProviders,
) -> Result<AgentdMatrixGateway, MatrixGatewayStartupError> {
    if let Some(missing) = MatrixGatewayProviderKind::ALL
        .into_iter()
        .find(|provider| !providers.has(*provider))
    {
        return Err(MatrixGatewayStartupError::MissingProvider(missing));
    }
    Ok(AgentdMatrixGateway {
        identity: take(&mut providers.identity),
        gateway: take(&mut providers.gateway),
        delivery: take(&mut providers.delivery),
        trusted_clock: take(&mut providers.trusted_clock),
    })
}

fn take<T: ?Sized>(provider: &mut Option<Arc<T>>) -> Arc<T> {
    provider
        .take()
        .expect("Matrix gateway provider checked before composition")
}

pub struct AgentdMatrixGateway {
    identity: Arc<dyn MatrixGatewayIdentityPort>,
    gateway: Arc<dyn MatrixGatewayPort>,
    delivery: Arc<dyn MatrixGatewayDeliveryPort>,
    trusted_clock: Arc<dyn Clock>,
}

impl fmt::Debug for AgentdMatrixGateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentdMatrixGateway")
            .field("providers", &"[CONFIGURED]")
            .finish()
    }
}

impl AgentdMatrixGateway {
    pub async fn ingest(
        &self,
        event: MatrixInboundTransportEvent,
    ) -> Result<MatrixCommandReceipt, MatrixGatewayError> {
        let observed_at = self.trusted_clock.now_unix();
        if observed_at < 0 {
            return Err(MatrixGatewayError::Unavailable(
                "trusted Matrix gateway clock returned invalid time".to_string(),
            ));
        }
        let identity = self
            .identity
            .authenticate_matrix_source(&event.provenance)
            .await?;
        let command = normalize_command(&event.body, &event.attachments)?;
        self.gateway
            .accept_command(&MatrixGatewayCommandRequest {
                provenance: event.provenance,
                identity,
                binding_ref: event.binding_ref,
                snapshot_ref: event.snapshot_ref,
                command,
                observed_at,
            })
            .await
    }

    pub async fn deliver_pending(
        &self,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> Result<usize, MatrixGatewayError> {
        let records = self.gateway.outbox_after(after_sequence, limit).await?;
        let mut delivered = 0;
        for record in records {
            self.delivery.deliver_summary(&record).await?;
            let delivered_at = self.trusted_clock.now_unix();
            if delivered_at < record.created_at {
                return Err(MatrixGatewayError::Unavailable(
                    "trusted Matrix delivery clock precedes outbox creation".to_string(),
                ));
            }
            self.gateway
                .mark_outbox_delivered(&record.outbox_id, delivered_at)
                .await?;
            delivered += 1;
        }
        Ok(delivered)
    }
}

pub fn normalize_command(
    body: &str,
    attachments: &[MatrixInboundAttachment],
) -> Result<NormalizedMatrixCommand, MatrixGatewayError> {
    if body.len() > 16 * 1024 || attachments.len() > 32 {
        return Err(MatrixGatewayError::Invalid(
            "Matrix command input exceeds bounds".to_string(),
        ));
    }
    let mut tokens = body.split_whitespace();
    let prefix = tokens.next().unwrap_or_default();
    let verb = tokens.next().unwrap_or_default();
    let class = match (prefix, verb) {
        ("/agentd", "execute") | ("/run", "start") => MatrixCommandClass::Execute,
        ("/agentd" | "/run", "status") => MatrixCommandClass::Status,
        ("/agentd" | "/run", "cancel") => MatrixCommandClass::Cancel,
        _ => {
            return Err(MatrixGatewayError::Invalid(
                "unsupported Matrix command".to_string(),
            ));
        }
    };
    let arguments = tokens.map(str::to_string).collect::<Vec<_>>();
    if arguments.len() > 64 || arguments.iter().any(|argument| argument.len() > 1024) {
        return Err(MatrixGatewayError::Invalid(
            "Matrix command arguments exceed bounds".to_string(),
        ));
    }
    let attachments = attachments
        .iter()
        .map(|attachment| {
            if attachment.bytes.is_empty()
                || attachment.bytes.len() > 100 * 1024 * 1024
                || attachment.media_type.trim().is_empty()
                || attachment.media_type.len() > 256
            {
                return Err(MatrixGatewayError::Invalid(
                    "Matrix attachment exceeds bounds".to_string(),
                ));
            }
            Ok(MatrixAttachmentRef {
                content_sha256: hex::encode(Sha256::digest(&attachment.bytes)),
                size_bytes: u64::try_from(attachment.bytes.len()).map_err(|_| {
                    MatrixGatewayError::Invalid("Matrix attachment size overflow".to_string())
                })?,
                media_type: attachment.media_type.trim().to_string(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let command_sha256 = {
        let value = serde_json::json!({
            "class": class,
            "arguments": arguments,
            "attachments": attachments,
        });
        hex::encode(Sha256::digest(
            serde_json::to_vec(&value)
                .map_err(|error| MatrixGatewayError::Invalid(error.to_string()))?,
        ))
    };
    Ok(NormalizedMatrixCommand {
        class,
        arguments,
        attachments,
        command_sha256,
    })
}
