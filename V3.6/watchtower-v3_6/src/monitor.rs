use chia_protocol::Coin;
use clvmr::{Allocator, NodePtr, SExp, serde::node_from_bytes};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use xhub_protocol_v3_6::{
    Bytes32, RecoveryPackage, StateZero, closing_state_hash, one_arg_puzzle_hash,
};
use xhub_puzzles_v3_6::{
    ChallengeSimulation, ClosingCoinKind, module_hashes, simulate_challenge,
    simulate_state_zero_challenge,
};

use crate::rpc::WatchtowerChainProvider;
use crate::{WatchtowerError, WatchtowerStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainPeak {
    pub height: u64,
    pub header_hash: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedCoin {
    pub coin_id: Bytes32,
    pub parent_coin_id: Bytes32,
    pub puzzle_hash: Bytes32,
    pub amount: u64,
    pub birth_height: u64,
    pub spent_height: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosingObservation {
    pub network_id: Bytes32,
    pub synced: bool,
    pub peak: ChainPeak,
    pub funding_coin: ObservedCoin,
    pub closing_coin: Option<ObservedCoin>,
    pub closing_coin_kind: Option<ClosingCoinKind>,
    pub current_state_sequence: Option<u64>,
    pub current_checkpoint_hash: Option<Bytes32>,
    pub initial_birth_height: Option<u64>,
    pub challenge_deadline_height: Option<u64>,
    pub terminal_finalized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MonitorAction {
    FundingOpen,
    ClosingCurrent,
    ChallengePlanned,
    ChallengeAlreadyPlanned,
    DeadlinePassed,
    Finalized,
    ReorgPending,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorDecision {
    pub action: MonitorAction,
    pub funding_coin_id: String,
    pub peak_height: Option<u64>,
    pub current_state_sequence: Option<u64>,
    pub latest_state_sequence: Option<u64>,
    pub challenge_deadline_height: Option<u64>,
    pub detail: String,
    pub spend_bundle_created: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
    pub challenge: Option<ChallengeSimulation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedChallengePlan {
    pub closing_coin_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub current_state_sequence: u64,
    pub latest_state_sequence: u64,
    pub challenge_deadline_height: u64,
    pub status: String,
    pub attempt_count: u64,
    pub next_retry_height: Option<u64>,
    pub last_error: Option<String>,
    pub simulation: ChallengeSimulation,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MonitorError {
    #[error("chain state is unknown: {0}")]
    Unknown(String),
    #[error("chain observation is invalid: {0}")]
    InvalidObservation(String),
    #[error("Closing Coin does not match the claimed V3.6 state: {0}")]
    ClosingMismatch(String),
    #[error("challenge simulation failed: {0}")]
    ChallengeInvalid(String),
}

impl WatchtowerStore {
    pub fn observe_chain(
        &mut self,
        funding_coin_id: Bytes32,
        observation: std::result::Result<ClosingObservation, MonitorError>,
        now: u64,
    ) -> crate::Result<MonitorDecision> {
        let observation = match observation {
            Ok(value) => value,
            Err(error) => {
                let decision = MonitorDecision {
                    action: MonitorAction::Unknown,
                    funding_coin_id: hex::encode(funding_coin_id),
                    peak_height: None,
                    current_state_sequence: None,
                    latest_state_sequence: None,
                    challenge_deadline_height: None,
                    detail: error.to_string(),
                    spend_bundle_created: false,
                    broadcast_ready: false,
                    chain_broadcast: false,
                    challenge: None,
                };
                self.persist_monitor_decision(funding_coin_id, None, &decision, now)?;
                self.mark_offline_recheck_required(
                    funding_coin_id,
                    "chain state became unknown; offline preparation requires a fresh snapshot",
                    now,
                )?;
                return Ok(decision);
            }
        };
        if observation.funding_coin.coin_id != funding_coin_id {
            return Err(WatchtowerError::Invalid(
                "observation Funding Coin does not match the requested monitor target".into(),
            ));
        }
        let decision = self.evaluate_observation(&observation)?;
        self.persist_monitor_decision(funding_coin_id, Some(&observation.peak), &decision, now)?;
        self.reconcile_offline_preparation(&observation, &decision, now)?;
        if decision.action == MonitorAction::ChallengePlanned {
            let closing_coin_id = observation
                .closing_coin
                .as_ref()
                .ok_or_else(|| WatchtowerError::Corrupt("challenge omitted Closing Coin".into()))?
                .coin_id;
            let simulation = decision.challenge.as_ref().ok_or_else(|| {
                WatchtowerError::Corrupt("challenge decision omitted simulation".into())
            })?;
            self.persist_challenge_plan(closing_coin_id, simulation, now)?;
        }
        Ok(decision)
    }

    pub(crate) fn evaluate_observation(
        &self,
        observation: &ClosingObservation,
    ) -> crate::Result<MonitorDecision> {
        validate_observation(observation)
            .map_err(|error| WatchtowerError::Invalid(error.to_string()))?;
        let funding_coin_id = observation.funding_coin.coin_id;
        let latest = self.latest_package(funding_coin_id)?;
        bind_funding_coin(&latest, observation)
            .map_err(|error| WatchtowerError::Invalid(error.to_string()))?;
        let latest_sequence = latest.official_state.checkpoint.state_sequence;
        let base = |action, detail: String| MonitorDecision {
            action,
            funding_coin_id: hex::encode(funding_coin_id),
            peak_height: Some(observation.peak.height),
            current_state_sequence: observation.current_state_sequence,
            latest_state_sequence: Some(latest_sequence),
            challenge_deadline_height: observation.challenge_deadline_height,
            detail,
            spend_bundle_created: false,
            broadcast_ready: false,
            chain_broadcast: false,
            challenge: None,
        };
        let Some(closing_coin) = &observation.closing_coin else {
            return if observation.funding_coin.spent_height.is_none() {
                let prior = self.last_monitor_action(funding_coin_id)?;
                if prior.as_deref().is_some_and(|value| {
                    matches!(
                        value,
                        "ClosingCurrent"
                            | "ChallengePlanned"
                            | "ChallengeAlreadyPlanned"
                            | "DeadlinePassed"
                    )
                }) {
                    Ok(base(
                        MonitorAction::ReorgPending,
                        "previously observed Closing Coin disappeared after a chain reorganization"
                            .into(),
                    ))
                } else {
                    Ok(base(
                        MonitorAction::FundingOpen,
                        "Funding Coin remains unspent".into(),
                    ))
                }
            } else {
                Ok(base(
                    MonitorAction::ReorgPending,
                    "Funding Coin is spent but its expected Closing Coin is not confirmed".into(),
                ))
            };
        };
        let current_sequence = observation.current_state_sequence.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted current state sequence".into())
        })?;
        let current = if current_sequence == 0 {
            verify_state_zero_closing_coin(&latest, observation)
                .map_err(|error| WatchtowerError::Invalid(error.to_string()))?;
            None
        } else {
            let package = self.package(funding_coin_id, current_sequence)?;
            verify_closing_coin(&package, observation)
                .map_err(|error| WatchtowerError::Invalid(error.to_string()))?;
            Some(package)
        };
        if observation.terminal_finalized {
            return Ok(base(
                MonitorAction::Finalized,
                "confirmed Closing Coin was spent through FINALIZE".into(),
            ));
        }
        if closing_coin.spent_height.is_some() {
            return Ok(base(
                MonitorAction::ReorgPending,
                "observed Closing Coin is already spent; follow its confirmed child before acting"
                    .into(),
            ));
        }
        if current_sequence >= latest_sequence {
            return Ok(base(
                MonitorAction::ClosingCurrent,
                "confirmed Closing Coin already commits to the latest known state".into(),
            ));
        }
        let deadline = observation.challenge_deadline_height.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted challenge deadline".into())
        })?;
        if observation.peak.height >= deadline {
            return Ok(base(
                MonitorAction::DeadlinePassed,
                "challenge deadline has been reached; no CHALLENGE may be constructed".into(),
            ));
        }
        if self.challenge_plan(closing_coin.coin_id)?.is_some() {
            return Ok(base(
                MonitorAction::ChallengeAlreadyPlanned,
                "an idempotent challenge plan already exists for this Closing Coin".into(),
            ));
        }
        let kind = observation.closing_coin_kind.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted coin kind".into())
        })?;
        let initial_birth = observation.initial_birth_height.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted initial birth height".into())
        })?;
        let simulation = match current {
            Some(current) => simulate_challenge(&current, &latest, kind, initial_birth, deadline),
            None if kind == ClosingCoinKind::Initial => {
                simulate_state_zero_challenge(&latest, initial_birth, deadline)
            }
            None => Err("State 0 may only appear in the Initial Closing Coin".into()),
        }
        .map_err(|error| {
            WatchtowerError::Invalid(MonitorError::ChallengeInvalid(error).to_string())
        })?;
        let mut decision = base(
            MonitorAction::ChallengePlanned,
            "higher complete RecoveryPackage verified; non-broadcast CHALLENGE plan persisted"
                .into(),
        );
        decision.challenge = Some(simulation);
        Ok(decision)
    }

    fn persist_monitor_decision(
        &self,
        funding_coin_id: Bytes32,
        peak: Option<&ChainPeak>,
        decision: &MonitorDecision,
        now: u64,
    ) -> crate::Result<()> {
        self.connection.execute(
            "INSERT INTO v36_chain_monitor_state (
               funding_coin_id, peak_height, peak_header_hash, action, detail, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(funding_coin_id) DO UPDATE SET
               peak_height=excluded.peak_height,
               peak_header_hash=excluded.peak_header_hash,
               action=excluded.action,
               detail=excluded.detail,
               updated_at=excluded.updated_at",
            params![
                funding_coin_id.as_slice(),
                peak.map(|value| super::to_i64(value.height)).transpose()?,
                peak.map(|value| value.header_hash.to_vec()),
                format!("{:?}", decision.action),
                decision.detail,
                super::to_i64(now)?,
            ],
        )?;
        Ok(())
    }

    fn last_monitor_action(&self, funding_coin_id: Bytes32) -> crate::Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT action FROM v36_chain_monitor_state WHERE funding_coin_id = ?1",
                [funding_coin_id.as_slice()],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn persist_challenge_plan(
        &self,
        closing_coin_id: Bytes32,
        simulation: &ChallengeSimulation,
        now: u64,
    ) -> crate::Result<()> {
        let simulation_json = serde_json::to_string(simulation)
            .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?;
        let funding_coin_id = parse_hex32(&simulation.funding_coin_id)
            .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?;
        self.connection.execute(
            "INSERT OR IGNORE INTO v36_challenge_plans (
               closing_coin_id, funding_coin_id, current_state_sequence,
               latest_state_sequence, challenge_deadline_height, simulation_json,
               status, attempt_count, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'SIMULATED_ONLY', 0, ?7, ?7)",
            params![
                closing_coin_id.as_slice(),
                funding_coin_id.as_slice(),
                super::to_i64(simulation.current_state_sequence)?,
                super::to_i64(simulation.latest_state_sequence)?,
                super::to_i64(simulation.challenge_deadline_height)?,
                simulation_json,
                super::to_i64(now)?,
            ],
        )?;
        Ok(())
    }

    pub fn challenge_plan(
        &self,
        closing_coin_id: Bytes32,
    ) -> crate::Result<Option<PersistedChallengePlan>> {
        let row = self
            .connection
            .query_row(
                "SELECT funding_coin_id, current_state_sequence, latest_state_sequence,
                    challenge_deadline_height, status, attempt_count, next_retry_height,
                    last_error, simulation_json
             FROM v36_challenge_plans WHERE closing_coin_id = ?1",
                [closing_coin_id.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| {
            Ok(PersistedChallengePlan {
                closing_coin_id,
                funding_coin_id: super::bytes32(row.0, "challenge funding coin id")?,
                current_state_sequence: super::from_i64(row.1, "current state sequence")?,
                latest_state_sequence: super::from_i64(row.2, "latest state sequence")?,
                challenge_deadline_height: super::from_i64(row.3, "challenge deadline")?,
                status: row.4,
                attempt_count: super::from_i64(row.5, "attempt count")?,
                next_retry_height: row
                    .6
                    .map(|value| super::from_i64(value, "next retry height"))
                    .transpose()?,
                last_error: row.7,
                simulation: serde_json::from_str(&row.8)
                    .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?,
            })
        })
        .transpose()
    }

    pub fn record_simulated_broadcast_failure(
        &self,
        closing_coin_id: Bytes32,
        current_height: u64,
        retry_after_blocks: u64,
        error: &str,
        now: u64,
    ) -> crate::Result<PersistedChallengePlan> {
        if error.is_empty() || error.len() > 1024 {
            return Err(WatchtowerError::Invalid(
                "broadcast error must contain 1..=1024 bytes".into(),
            ));
        }
        let plan = self
            .challenge_plan(closing_coin_id)?
            .ok_or_else(|| WatchtowerError::Invalid("challenge plan was not found".into()))?;
        if current_height >= plan.challenge_deadline_height {
            return Err(WatchtowerError::Invalid(
                "challenge deadline has been reached".into(),
            ));
        }
        let next_retry = current_height
            .checked_add(retry_after_blocks.max(1))
            .ok_or_else(|| WatchtowerError::Invalid("retry height overflow".into()))?
            .min(plan.challenge_deadline_height - 1);
        self.connection.execute(
            "UPDATE v36_challenge_plans SET
               status='RETRY_SCHEDULED', attempt_count=attempt_count + 1,
               next_retry_height=?2, last_error=?3, updated_at=?4
             WHERE closing_coin_id=?1",
            params![
                closing_coin_id.as_slice(),
                super::to_i64(next_retry)?,
                error,
                super::to_i64(now)?,
            ],
        )?;
        self.challenge_plan(closing_coin_id)?.ok_or_else(|| {
            WatchtowerError::Corrupt("challenge plan disappeared after retry update".into())
        })
    }

    pub fn poll_chain<P: WatchtowerChainProvider>(
        &mut self,
        provider: &P,
        funding_coin_id: Bytes32,
        now: u64,
    ) -> crate::Result<MonitorDecision> {
        let observation = self.build_observation(provider, funding_coin_id);
        self.observe_chain(funding_coin_id, observation, now)
    }

    pub(crate) fn build_observation<P: WatchtowerChainProvider>(
        &self,
        provider: &P,
        funding_coin_id: Bytes32,
    ) -> std::result::Result<ClosingObservation, MonitorError> {
        let view = provider.chain_view()?;
        if !view.synced {
            return Err(MonitorError::Unknown("chain source is not synced".into()));
        }
        let funding = provider.coin(funding_coin_id)?.ok_or_else(|| {
            MonitorError::Unknown("Funding Coin is missing from the current chain view".into())
        })?;
        if funding.spent_height.is_none() {
            return Ok(ClosingObservation {
                network_id: view.network_id,
                synced: view.synced,
                peak: view.peak,
                funding_coin: funding,
                closing_coin: None,
                closing_coin_kind: None,
                current_state_sequence: None,
                current_checkpoint_hash: None,
                initial_birth_height: None,
                challenge_deadline_height: None,
                terminal_finalized: false,
            });
        }
        let funding_spent_height = funding.spent_height.expect("checked spent height");
        let funding_spend = provider.coin_spend(funding_coin_id, funding_spent_height)?;
        require_spend_reveal(&funding_spend.puzzle_reveal, funding.puzzle_hash, "Funding")?;
        let funding_solution = parse_funding_solution(&funding_spend.solution)?;
        if funding_solution.funding_coin_id != funding_coin_id {
            return Err(MonitorError::ClosingMismatch(
                "Funding solution contains a different Funding Coin ID".into(),
            ));
        }
        let funding_state = funding_solution.state;
        let mut package = self
            .latest_package(funding_coin_id)
            .map_err(|error| MonitorError::Unknown(error.to_string()))?;
        if funding_state.sequence == 0 {
            require_state_zero_binding(&package, &funding_state)?;
        } else {
            package = self
                .package(funding_coin_id, funding_state.sequence)
                .map_err(|error| MonitorError::Unknown(error.to_string()))?;
            require_checkpoint_binding(&package, &funding_state)?;
        }
        let hashes = module_hashes();
        let terms_hash = package
            .channel_terms
            .hash()
            .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?;
        let checkpoint_hash = if funding_state.sequence == 0 {
            StateZero::new(&package.channel_terms)
                .and_then(|state| state.hash(&package.channel_terms, &funding_coin_id))
                .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?
        } else {
            package
                .official_state
                .checkpoint
                .hash(&package.channel_terms)
                .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?
        };
        let initial_commitment = closing_state_hash(
            &package.channel_terms.network_id,
            &funding_coin_id,
            &terms_hash,
            &[0; 8],
            &checkpoint_hash,
        );
        let initial_hash = one_arg_puzzle_hash(hashes.initial_closing, &initial_commitment);
        let initial_id = Coin::new(
            funding_coin_id.into(),
            initial_hash.into(),
            package.funding_amount,
        )
        .coin_id()
        .to_bytes();
        let Some(mut closing) = provider.coin(initial_id)? else {
            return Ok(ClosingObservation {
                network_id: view.network_id,
                synced: view.synced,
                peak: view.peak,
                funding_coin: funding,
                closing_coin: None,
                closing_coin_kind: None,
                current_state_sequence: None,
                current_checkpoint_hash: None,
                initial_birth_height: None,
                challenge_deadline_height: None,
                terminal_finalized: false,
            });
        };
        let initial_birth = closing.birth_height;
        let deadline = initial_birth
            .checked_add(package.channel_terms.challenge_blocks)
            .ok_or_else(|| {
                MonitorError::InvalidObservation("challenge deadline overflow".into())
            })?;
        let mut kind = ClosingCoinKind::Initial;
        for _ in 0..64 {
            let current_sequence = if kind == ClosingCoinKind::Initial {
                funding_state.sequence
            } else {
                package.official_state.checkpoint.state_sequence
            };
            let current_hash = if current_sequence == 0 {
                StateZero::new(&package.channel_terms)
                    .and_then(|state| state.hash(&package.channel_terms, &funding_coin_id))
                    .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?
            } else {
                package
                    .official_state
                    .checkpoint
                    .hash(&package.channel_terms)
                    .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?
            };
            let Some(spent_height) = closing.spent_height else {
                return Ok(ClosingObservation {
                    network_id: view.network_id,
                    synced: view.synced,
                    peak: view.peak,
                    funding_coin: funding,
                    closing_coin: Some(closing),
                    closing_coin_kind: Some(kind),
                    current_state_sequence: Some(current_sequence),
                    current_checkpoint_hash: Some(current_hash),
                    initial_birth_height: Some(initial_birth),
                    challenge_deadline_height: Some(deadline),
                    terminal_finalized: false,
                });
            };
            let spend = provider.coin_spend(closing.coin_id, spent_height)?;
            require_spend_reveal(&spend.puzzle_reveal, closing.puzzle_hash, "Closing")?;
            let transition = parse_closing_solution(&spend.solution, kind)?;
            if current_sequence == 0 {
                require_state_zero_binding(&package, &transition.current_state)?;
            } else {
                require_checkpoint_binding(&package, &transition.current_state)?;
            }
            if transition.mode == 2 {
                return Ok(ClosingObservation {
                    network_id: view.network_id,
                    synced: view.synced,
                    peak: view.peak,
                    funding_coin: funding,
                    closing_coin: Some(closing),
                    closing_coin_kind: Some(kind),
                    current_state_sequence: Some(current_sequence),
                    current_checkpoint_hash: Some(current_hash),
                    initial_birth_height: Some(initial_birth),
                    challenge_deadline_height: Some(deadline),
                    terminal_finalized: true,
                });
            }
            if transition.mode != 1 || transition.deadline != deadline {
                return Err(MonitorError::ClosingMismatch(
                    "Closing spend has an invalid mode or reset deadline".into(),
                ));
            }
            let next = self
                .package(funding_coin_id, transition.new_sequence)
                .map_err(|error| MonitorError::Unknown(error.to_string()))?;
            require_checkpoint_binding(&next, &transition.new_state)?;
            let next_hash = next
                .official_state
                .checkpoint
                .hash(&next.channel_terms)
                .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?;
            let commitment = closing_state_hash(
                &next.channel_terms.network_id,
                &funding_coin_id,
                &terms_hash,
                &deadline.to_be_bytes(),
                &next_hash,
            );
            let puzzle_hash = one_arg_puzzle_hash(hashes.subsequent_closing, &commitment);
            let child_id = Coin::new(
                closing.coin_id.into(),
                puzzle_hash.into(),
                next.funding_amount,
            )
            .coin_id()
            .to_bytes();
            closing = provider.coin(child_id)?.ok_or_else(|| {
                MonitorError::Unknown("confirmed CHALLENGE child Closing Coin is missing".into())
            })?;
            package = next;
            kind = ClosingCoinKind::Subsequent;
        }
        Err(MonitorError::Unknown(
            "Closing lineage exceeded the 64-transition safety limit".into(),
        ))
    }
}

fn validate_observation(observation: &ClosingObservation) -> Result<(), MonitorError> {
    if !observation.synced {
        return Err(MonitorError::Unknown("chain source is not synced".into()));
    }
    if observation.peak.height == 0 || observation.funding_coin.birth_height == 0 {
        return Err(MonitorError::InvalidObservation(
            "coin and peak heights must be positive".into(),
        ));
    }
    if observation.funding_coin.birth_height > observation.peak.height {
        return Err(MonitorError::InvalidObservation(
            "Funding Coin birth height exceeds peak".into(),
        ));
    }
    if let Some(closing) = &observation.closing_coin {
        if closing.birth_height > observation.peak.height
            || closing.amount != observation.funding_coin.amount
        {
            return Err(MonitorError::InvalidObservation(
                "Closing Coin amount or birth height is invalid".into(),
            ));
        }
        match observation.closing_coin_kind {
            Some(ClosingCoinKind::Initial)
                if closing.parent_coin_id != observation.funding_coin.coin_id
                    || observation.funding_coin.spent_height != Some(closing.birth_height) =>
            {
                return Err(MonitorError::InvalidObservation(
                    "Initial Closing Coin parent or birth height is invalid".into(),
                ));
            }
            Some(ClosingCoinKind::Subsequent)
                if closing.parent_coin_id == observation.funding_coin.coin_id
                    || observation.initial_birth_height
                        != observation.funding_coin.spent_height
                    || closing.birth_height
                        < observation.initial_birth_height.unwrap_or(u64::MAX) =>
            {
                return Err(MonitorError::InvalidObservation(
                    "Subsequent Closing Coin does not preserve the initial lineage height".into(),
                ));
            }
            Some(_) => {}
            None => {
                return Err(MonitorError::InvalidObservation(
                    "Closing Coin kind is missing".into(),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedState {
    sequence: u64,
    previous_checkpoint_hash: Bytes32,
    manifest_root: Bytes32,
    entry_count: u64,
    reserved_total: u64,
    user_remainder: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedFundingSolution {
    funding_coin_id: Bytes32,
    state: ParsedState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTransition {
    mode: u64,
    deadline: u64,
    new_sequence: u64,
    current_state: ParsedState,
    new_state: ParsedState,
}

fn parse_funding_solution(bytes: &[u8]) -> Result<ParsedFundingSolution, MonitorError> {
    let (allocator, fields) = decode_list(bytes, "Funding solution")?;
    if fields.len() != 8 {
        return Err(MonitorError::ClosingMismatch(
            "Funding solution must contain exactly 8 fields".into(),
        ));
    }
    Ok(ParsedFundingSolution {
        funding_coin_id: atom_bytes32(&allocator, fields[0], "funding_coin_id")?,
        state: parsed_state(&allocator, &fields, 1)?,
    })
}

fn parse_closing_solution(
    bytes: &[u8],
    kind: ClosingCoinKind,
) -> Result<ParsedTransition, MonitorError> {
    let (allocator, fields) = decode_list(bytes, "Closing solution")?;
    let (expected_len, deadline_index, current_sequence_index, new_sequence_index) = match kind {
        ClosingCoinKind::Initial => (32, 2, 18, 25),
        ClosingCoinKind::Subsequent => (31, 1, 17, 24),
    };
    if fields.len() != expected_len {
        return Err(MonitorError::ClosingMismatch(format!(
            "Closing solution must contain exactly {expected_len} fields"
        )));
    }
    let new_state = parsed_state(&allocator, &fields, new_sequence_index)?;
    Ok(ParsedTransition {
        mode: atom_u64_strict(&allocator, fields[0], "mode", 1)?,
        deadline: atom_u64_strict(
            &allocator,
            fields[deadline_index],
            "challenge_deadline_height",
            8,
        )?,
        new_sequence: new_state.sequence,
        current_state: parsed_state(&allocator, &fields, current_sequence_index)?,
        new_state,
    })
}

fn parsed_state(
    allocator: &Allocator,
    fields: &[NodePtr],
    start: usize,
) -> Result<ParsedState, MonitorError> {
    Ok(ParsedState {
        sequence: atom_u64_strict(allocator, fields[start], "state_sequence", 8)?,
        previous_checkpoint_hash: atom_bytes32(
            allocator,
            fields[start + 1],
            "previous_checkpoint_hash",
        )?,
        manifest_root: atom_bytes32(allocator, fields[start + 2], "manifest_root")?,
        entry_count: atom_u64_strict(allocator, fields[start + 3], "entry_count", 8)?,
        reserved_total: atom_u64_strict(allocator, fields[start + 4], "reserved_total", 8)?,
        user_remainder: atom_u64_strict(allocator, fields[start + 5], "user_remainder", 8)?,
    })
}

fn require_checkpoint_binding(
    package: &RecoveryPackage,
    state: &ParsedState,
) -> Result<(), MonitorError> {
    let checkpoint = &package.official_state.checkpoint;
    if checkpoint.state_sequence != state.sequence
        || checkpoint.previous_checkpoint_hash != state.previous_checkpoint_hash
        || checkpoint.manifest_root != state.manifest_root
        || checkpoint.entry_count != state.entry_count
        || checkpoint.reserved_total != state.reserved_total
        || checkpoint.user_remainder != state.user_remainder
    {
        return Err(MonitorError::ClosingMismatch(
            "chain spend state does not match the persisted RecoveryPackage".into(),
        ));
    }
    Ok(())
}

fn require_state_zero_binding(
    package: &RecoveryPackage,
    state: &ParsedState,
) -> Result<(), MonitorError> {
    let zero = StateZero::new(&package.channel_terms)
        .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?;
    if state.sequence != 0
        || state.previous_checkpoint_hash != [0; 32]
        || state.manifest_root != zero.manifest_root
        || state.entry_count != 0
        || state.reserved_total != 0
        || state.user_remainder != zero.user_remainder
    {
        return Err(MonitorError::ClosingMismatch(
            "chain spend does not encode the canonical State 0".into(),
        ));
    }
    Ok(())
}

fn decode_list(bytes: &[u8], label: &str) -> Result<(Allocator, Vec<NodePtr>), MonitorError> {
    let mut allocator = Allocator::new();
    let root = node_from_bytes(&mut allocator, bytes).map_err(|error| {
        MonitorError::ClosingMismatch(format!("{label} is invalid CLVM: {error:?}"))
    })?;
    let fields = proper_list(&allocator, root)
        .ok_or_else(|| MonitorError::ClosingMismatch(format!("{label} is not a proper list")))?;
    Ok((allocator, fields))
}

fn proper_list(allocator: &Allocator, mut node: NodePtr) -> Option<Vec<NodePtr>> {
    let mut fields = Vec::new();
    loop {
        match allocator.sexp(node) {
            SExp::Pair(first, rest) => {
                fields.push(first);
                node = rest;
            }
            SExp::Atom if allocator.atom(node).is_empty() => return Some(fields),
            SExp::Atom => return None,
        }
    }
}

fn atom_u64_strict(
    allocator: &Allocator,
    node: NodePtr,
    field: &str,
    length: usize,
) -> Result<u64, MonitorError> {
    let atom = allocator.atom(node);
    if atom.len() != length || length > 8 {
        return Err(MonitorError::ClosingMismatch(format!(
            "{field} must be a fixed {length}-byte atom"
        )));
    }
    Ok(atom
        .as_ref()
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte)))
}

fn atom_bytes32(
    allocator: &Allocator,
    node: NodePtr,
    field: &str,
) -> Result<Bytes32, MonitorError> {
    allocator
        .atom(node)
        .as_ref()
        .try_into()
        .map_err(|_| MonitorError::ClosingMismatch(format!("{field} must be a 32-byte atom")))
}

fn bind_funding_coin(
    package: &RecoveryPackage,
    observation: &ClosingObservation,
) -> Result<(), MonitorError> {
    let mut allocator = clvmr::Allocator::new();
    let node = clvmr::serde::node_from_bytes(&mut allocator, &package.funding_puzzle_reveal)
        .map_err(|error| {
            MonitorError::InvalidObservation(format!("Funding puzzle reveal: {error:?}"))
        })?;
    let puzzle_hash = clvm_utils::tree_hash(&allocator, node).to_bytes();
    if observation.network_id != package.channel_terms.network_id
        || observation.funding_coin.puzzle_hash != puzzle_hash
        || observation.funding_coin.amount != package.funding_amount
        || observation.funding_coin.coin_id != package.funding_coin_id
    {
        return Err(MonitorError::InvalidObservation(
            "Funding Coin does not bind to the persisted RecoveryPackage".into(),
        ));
    }
    Ok(())
}

fn require_spend_reveal(
    reveal: &[u8],
    expected_puzzle_hash: Bytes32,
    label: &str,
) -> Result<(), MonitorError> {
    let mut allocator = Allocator::new();
    let node = node_from_bytes(&mut allocator, reveal).map_err(|error| {
        MonitorError::ClosingMismatch(format!("{label} puzzle reveal is invalid: {error:?}"))
    })?;
    if clvm_utils::tree_hash(&allocator, node).to_bytes() != expected_puzzle_hash {
        return Err(MonitorError::ClosingMismatch(format!(
            "{label} puzzle reveal does not match its CoinRecord"
        )));
    }
    Ok(())
}

fn verify_closing_coin(
    package: &RecoveryPackage,
    observation: &ClosingObservation,
) -> Result<(), MonitorError> {
    let closing = observation
        .closing_coin
        .as_ref()
        .ok_or_else(|| MonitorError::InvalidObservation("Closing Coin is missing".into()))?;
    let kind = observation
        .closing_coin_kind
        .ok_or_else(|| MonitorError::InvalidObservation("Closing Coin kind is missing".into()))?;
    let checkpoint_hash = package
        .official_state
        .checkpoint
        .hash(&package.channel_terms)
        .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?;
    if observation.current_checkpoint_hash != Some(checkpoint_hash) {
        return Err(MonitorError::ClosingMismatch(
            "checkpoint hash mismatch".into(),
        ));
    }
    let deadline = match kind {
        ClosingCoinKind::Initial => [0; 8],
        ClosingCoinKind::Subsequent => observation
            .challenge_deadline_height
            .ok_or_else(|| MonitorError::InvalidObservation("deadline is missing".into()))?
            .to_be_bytes(),
    };
    let terms_hash = package
        .channel_terms
        .hash()
        .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?;
    let commitment = closing_state_hash(
        &package.channel_terms.network_id,
        &package.funding_coin_id,
        &terms_hash,
        &deadline,
        &checkpoint_hash,
    );
    let hashes = module_hashes();
    let module_hash = match kind {
        ClosingCoinKind::Initial => hashes.initial_closing,
        ClosingCoinKind::Subsequent => hashes.subsequent_closing,
    };
    let expected_puzzle_hash = one_arg_puzzle_hash(module_hash, &commitment);
    let expected_id = Coin::new(
        closing.parent_coin_id.into(),
        expected_puzzle_hash.into(),
        closing.amount,
    )
    .coin_id()
    .to_bytes();
    if closing.puzzle_hash != expected_puzzle_hash || closing.coin_id != expected_id {
        return Err(MonitorError::ClosingMismatch(
            "coin id or puzzle hash mismatch".into(),
        ));
    }
    Ok(())
}

fn verify_state_zero_closing_coin(
    latest: &RecoveryPackage,
    observation: &ClosingObservation,
) -> Result<(), MonitorError> {
    let closing = observation
        .closing_coin
        .as_ref()
        .ok_or_else(|| MonitorError::InvalidObservation("Closing Coin is missing".into()))?;
    if observation.closing_coin_kind != Some(ClosingCoinKind::Initial) {
        return Err(MonitorError::ClosingMismatch(
            "State 0 may only be committed by the Initial Closing Coin".into(),
        ));
    }
    let zero_hash = StateZero::new(&latest.channel_terms)
        .and_then(|state| state.hash(&latest.channel_terms, &latest.funding_coin_id))
        .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?;
    if observation.current_checkpoint_hash != Some(zero_hash) {
        return Err(MonitorError::ClosingMismatch(
            "State 0 hash mismatch".into(),
        ));
    }
    let terms_hash = latest
        .channel_terms
        .hash()
        .map_err(|error| MonitorError::ClosingMismatch(error.to_string()))?;
    let commitment = closing_state_hash(
        &latest.channel_terms.network_id,
        &latest.funding_coin_id,
        &terms_hash,
        &[0; 8],
        &zero_hash,
    );
    let puzzle_hash = one_arg_puzzle_hash(module_hashes().initial_closing, &commitment);
    let coin_id = Coin::new(
        closing.parent_coin_id.into(),
        puzzle_hash.into(),
        closing.amount,
    )
    .coin_id()
    .to_bytes();
    if closing.puzzle_hash != puzzle_hash || closing.coin_id != coin_id {
        return Err(MonitorError::ClosingMismatch(
            "State 0 Closing Coin ID or puzzle hash mismatch".into(),
        ));
    }
    Ok(())
}

fn parse_hex32(value: &str) -> Result<Bytes32, MonitorError> {
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| MonitorError::InvalidObservation(error.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| MonitorError::InvalidObservation("expected 32-byte hex value".into()))
}
