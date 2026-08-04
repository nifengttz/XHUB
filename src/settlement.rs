use chia_bls::{SecretKey, sign};
use chia_protocol::{Bytes32, Coin, SpendBundle};
use chia_puzzle_types::Memos;
use chia_sdk_driver::{SpendContext, StandardLayer};
use chia_sdk_test::sign_transaction;
use chia_sdk_types::Conditions;
use clvm_utils::ToTreeHash;
use thiserror::Error;

use crate::{
    ChainObservation, ChannelArgs, ChannelSolution, ChannelState, ChannelStore, PaymentVoucher,
    StateStoreError, coin_spend, is_reorged, refund_hash_for_funding_amount,
};

#[derive(Debug, Error)]
pub enum SettlementWorkflowError {
    #[error(transparent)]
    State(#[from] StateStoreError),
    #[error("failed to construct coin spend: {0}")]
    Spend(#[from] anyhow::Error),
    #[error("voucher is bound to another funding coin")]
    WrongFundingCoin,
    #[error("secret key does not match the channel user")]
    WrongUserKey,
    #[error("fee coin does not use the supplied key")]
    WrongFeeKey,
    #[error("fee coin amount must be positive")]
    InvalidFeeAmount,
    #[error("fee must be positive and no greater than the fee coin amount")]
    InvalidFee,
    #[error("channel has no persisted voucher")]
    MissingVoucher,
    #[error("expected state {expected:?}, found {actual:?}")]
    UnexpectedState {
        expected: ChannelState,
        actual: ChannelState,
    },
    #[error("confirmed funding outputs do not match the protocol settlement")]
    ConfirmationMismatch,
}

pub fn build_claim_bundle(
    funding_coin: Coin,
    args: &ChannelArgs,
    voucher: &PaymentVoucher,
) -> Result<SpendBundle, SettlementWorkflowError> {
    let commitment = &voucher.intent.commitment;
    if commitment.funding_coin_id != funding_coin.coin_id() {
        return Err(SettlementWorkflowError::WrongFundingCoin);
    }
    let funding_amount = commitment
        .merchant_amount
        .checked_add(commitment.user_remaining_amount)
        .ok_or(SettlementWorkflowError::ConfirmationMismatch)?;
    let solution = ChannelSolution::claim_for_funding_amount(
        commitment.funding_coin_id,
        commitment.invoice_hash,
        commitment.order_id,
        commitment.merchant_puzzle_hash,
        commitment.nonce,
        commitment.payment_expiry_height,
        funding_amount,
    )?;
    let spend = coin_spend(funding_coin, args, &solution)?;
    Ok(SpendBundle::new(
        vec![spend],
        voucher.aggregated_signature(),
    ))
}

pub fn build_refund_bundle(
    funding_coin: Coin,
    args: &ChannelArgs,
    user_secret_key: &SecretKey,
    agg_sig_me_additional_data: Bytes32,
) -> Result<SpendBundle, SettlementWorkflowError> {
    if user_secret_key.public_key() != args.user_public_key {
        return Err(SettlementWorkflowError::WrongUserKey);
    }
    let solution =
        ChannelSolution::refund_for_funding_amount(funding_coin.coin_id(), funding_coin.amount)?;
    let spend = coin_spend(funding_coin, args, &solution)?;
    let message = [
        refund_hash_for_funding_amount(args, funding_coin.coin_id(), funding_coin.amount).as_ref(),
        funding_coin.coin_id().as_ref(),
        agg_sig_me_additional_data.as_ref(),
    ]
    .concat();
    Ok(SpendBundle::new(
        vec![spend],
        sign(user_secret_key, message),
    ))
}

pub fn build_fee_bundle(
    fee_coin: Coin,
    fee_secret_key: &SecretKey,
) -> Result<SpendBundle, SettlementWorkflowError> {
    build_fee_bundle_with_change(
        fee_coin,
        fee_secret_key,
        fee_coin.amount,
        fee_coin.puzzle_hash,
    )
}

pub fn build_fee_bundle_with_change(
    fee_coin: Coin,
    fee_secret_key: &SecretKey,
    fee: u64,
    change_puzzle_hash: Bytes32,
) -> Result<SpendBundle, SettlementWorkflowError> {
    if fee_coin.amount == 0 {
        return Err(SettlementWorkflowError::InvalidFeeAmount);
    }
    if fee == 0 || fee > fee_coin.amount {
        return Err(SettlementWorkflowError::InvalidFee);
    }
    let layer = StandardLayer::new(fee_secret_key.public_key());
    let expected_puzzle_hash: Bytes32 = layer.tree_hash().into();
    if fee_coin.puzzle_hash != expected_puzzle_hash {
        return Err(SettlementWorkflowError::WrongFeeKey);
    }
    let mut context = SpendContext::new();
    let change = fee_coin.amount - fee;
    let conditions = if change == 0 {
        Conditions::new().reserve_fee(fee)
    } else {
        Conditions::new()
            .reserve_fee(fee)
            .create_coin(change_puzzle_hash, change, Memos::None)
    };
    layer
        .spend(&mut context, fee_coin, conditions)
        .map_err(anyhow::Error::from)?;
    let coin_spends = context.take();
    let signature = sign_transaction(&coin_spends, std::slice::from_ref(fee_secret_key))
        .map_err(anyhow::Error::from)?;
    Ok(SpendBundle::new(coin_spends, signature))
}

pub fn aggregate_fee_bundle(channel_bundle: SpendBundle, fee_bundle: SpendBundle) -> SpendBundle {
    SpendBundle::aggregate(&[channel_bundle, fee_bundle])
}

pub fn track_claim_submission(
    store: &mut ChannelStore,
    channel_id: Bytes32,
) -> Result<(), SettlementWorkflowError> {
    store.mark_claim_submitted(channel_id)?;
    Ok(())
}

pub fn track_refund_submission(
    store: &mut ChannelStore,
    channel_id: Bytes32,
) -> Result<(), SettlementWorkflowError> {
    store.mark_refund_submitted(channel_id)?;
    Ok(())
}

pub fn confirm_claim(
    store: &mut ChannelStore,
    channel_id: Bytes32,
    funding_coin_id: Bytes32,
    confirmed_children: &[Coin],
) -> Result<(), SettlementWorkflowError> {
    let record = store.load_channel(channel_id)?;
    if record.state != ChannelState::ClaimSubmitted {
        return Err(SettlementWorkflowError::UnexpectedState {
            expected: ChannelState::ClaimSubmitted,
            actual: record.state,
        });
    }
    let voucher = record
        .voucher
        .ok_or(SettlementWorkflowError::MissingVoucher)?;
    let commitment = voucher.intent.commitment;
    let expected = [
        (commitment.merchant_puzzle_hash, commitment.merchant_amount),
        (
            commitment.user_puzzle_hash,
            commitment.user_remaining_amount,
        ),
    ];
    if !outputs_match(funding_coin_id, confirmed_children, &expected) {
        return Err(SettlementWorkflowError::ConfirmationMismatch);
    }
    store.mark_settled(channel_id)?;
    Ok(())
}

pub fn confirm_refund(
    store: &mut ChannelStore,
    channel_id: Bytes32,
    funding_coin_id: Bytes32,
    user_puzzle_hash: Bytes32,
    confirmed_children: &[Coin],
) -> Result<(), SettlementWorkflowError> {
    let record = store.load_channel(channel_id)?;
    if record.state != ChannelState::RefundSubmitted {
        return Err(SettlementWorkflowError::UnexpectedState {
            expected: ChannelState::RefundSubmitted,
            actual: record.state,
        });
    }
    let funding_amount = record
        .merchant_amount
        .checked_add(record.user_remaining_amount)
        .ok_or(SettlementWorkflowError::ConfirmationMismatch)?;
    if !outputs_match(funding_coin_id, confirmed_children, &[(user_puzzle_hash, funding_amount)]) {
        return Err(SettlementWorkflowError::ConfirmationMismatch);
    }
    store.mark_refunded(channel_id)?;
    Ok(())
}

pub fn reconcile_chain_observation(
    store: &mut ChannelStore,
    channel_id: Bytes32,
    previous: Option<&ChainObservation>,
    current: &ChainObservation,
    fee: Option<u64>,
) -> Result<bool, SettlementWorkflowError> {
    let reorged = previous.is_some_and(|previous| is_reorged(previous, current));
    if reorged {
        match store.load_channel(channel_id)?.state {
            ChannelState::Settled => store.rollback_claim_after_reorg(channel_id)?,
            ChannelState::Refunded => store.rollback_refund_after_reorg(channel_id)?,
            _ => {}
        }
    }
    store.record_chain_observation(channel_id, current, fee, reorged)?;
    Ok(reorged)
}

fn outputs_match(funding_coin_id: Bytes32, actual: &[Coin], expected: &[(Bytes32, u64)]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .all(|coin| coin.parent_coin_info == funding_coin_id)
        && expected.iter().all(|(puzzle_hash, amount)| {
            actual
                .iter()
                .filter(|coin| coin.puzzle_hash == *puzzle_hash && coin.amount == *amount)
                .count()
                == 1
        })
}
