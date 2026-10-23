use crate::{
    env::BlockEnvironment,
    rpc::{CallFees, CallFeesError},
    tx_env_from_eip8141, EvmEnv,
};
use alloy_consensus::{TxEip8141, TxType};
use alloy_eips::eip8141::TransactionFees;
use alloy_primitives::{TxKind, U256};
use alloy_rpc_types_eth::request::{TransactionInputError, TransactionRequest};
use core::fmt::Debug;
use revm::{context::TxEnv, context_interface::either::Either};
use thiserror::Error;

/// Converts `self` into `T`.
///
/// Should create an executable transaction environment using [`TransactionRequest`].
pub trait TryIntoTxEnv<
    T,
    Spec = revm::primitives::hardfork::SpecId,
    BlockEnv = revm::context::BlockEnv,
>
{
    /// An associated error that can occur during the conversion.
    type Err;

    /// Performs the conversion.
    fn try_into_tx_env(self, evm_env: &EvmEnv<Spec, BlockEnv>) -> Result<T, Self::Err>;
}

/// An Ethereum specific transaction environment error than can occur during conversion from
/// [`TransactionRequest`].
#[derive(Debug, Error)]
pub enum EthTxEnvError {
    /// Error while decoding or validating transaction request fees.
    #[error(transparent)]
    CallFees(#[from] CallFeesError),
    /// Both data and input fields are set and not equal.
    #[error(transparent)]
    Input(#[from] TransactionInputError),
    /// An EIP-8141 request included fields which are not part of the frame
    /// transaction envelope and therefore cannot be silently normalized away.
    #[error("EIP-8141 transaction request contains non-canonical outer fields")]
    Eip8141InvalidOuterFields,
    /// A frame request must have both gas limits before execution.
    #[error("{0}")]
    Eip8141MissingLimit(&'static str),
}

impl<Spec, Block: BlockEnvironment> TryIntoTxEnv<TxEnv, Spec, Block> for TransactionRequest {
    type Err = EthTxEnvError;

    fn try_into_tx_env(self, evm_env: &EvmEnv<Spec, Block>) -> Result<TxEnv, Self::Err> {
        let tx_type = self.minimal_tx_type() as u8;

        // Ensure that if versioned hashes are set, they're not empty
        // EIP-8141 explicitly permits transactions without blobs. Alloy keeps an empty
        // `Some` here when round-tripping a frame transaction through `TransactionRequest`.
        if tx_type != TxType::Eip8141 as u8
            && self.blob_versioned_hashes.as_ref().is_some_and(|hashes| hashes.is_empty())
        {
            return Err(CallFeesError::BlobTransactionMissingBlobHashes.into());
        }

        let Self {
            from,
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input,
            nonce,
            nonce_keys,
            access_list,
            chain_id,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            authorization_list,
            frames,
            signatures,
            eip8141_fees,
            max_fee,
            ..
        } = self;

        let input = input.try_into_unique_input().map_err(EthTxEnvError::from)?.unwrap_or_default();

        if tx_type == TxType::Eip8141 as u8
            && (to.is_some()
                || gas_price.is_some()
                || gas.is_some()
                || value.is_some_and(|value| !value.is_zero())
                || !input.is_empty()
                || access_list.is_some()
                || authorization_list.is_some())
        {
            return Err(EthTxEnvError::Eip8141InvalidOuterFields);
        }

        if tx_type == TxType::Eip8141 as u8
            && (max_fee_per_gas.is_some() || max_fee_per_blob_gas.is_some())
        {
            return Err(EthTxEnvError::Eip8141InvalidOuterFields);
        }
        let chain_id = chain_id.unwrap_or(evm_env.cfg_env().chain_id);
        let caller = from.unwrap_or_default();
        let nonce = nonce.unwrap_or_default();
        if tx_type == TxType::Eip8141 as u8 {
            let tx = TxEip8141 {
                chain_id,
                nonce,
                nonce_keys,
                sender: caller,
                frames: frames
                    .unwrap_or_default()
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()
                    .map_err(EthTxEnvError::Eip8141MissingLimit)?,
                signatures: signatures.unwrap_or_default(),
                fees: TransactionFees {
                    max_priority_fee_per_gas: max_priority_fee_per_gas
                        .map(U256::from)
                        .or_else(|| eip8141_fees.map(|fees| fees.max_priority_fee_per_gas))
                        .unwrap_or_default(),
                    max_fee: max_fee
                        .or_else(|| eip8141_fees.map(|fees| fees.max_fee))
                        .unwrap_or(U256::MAX),
                },
                blob_versioned_hashes: blob_versioned_hashes.unwrap_or_default(),
            };
            return Ok(tx_env_from_eip8141(tx, &evm_env.cfg_env().gas_params));
        }

        let CallFees { max_priority_fee_per_gas, gas_price, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                gas_price.map(U256::from),
                max_fee_per_gas.map(U256::from),
                max_priority_fee_per_gas.map(U256::from),
                U256::from(evm_env.block_env().basefee()),
                blob_versioned_hashes.as_deref(),
                max_fee_per_blob_gas.map(U256::from),
                evm_env.block_env().blob_gasprice().map(U256::from),
            )?;
        let gas_limit = gas.unwrap_or(evm_env.block_env().gas_limit());

        let env = TxEnv {
            tx_type,
            gas_limit,
            nonce,
            caller,
            gas_price: gas_price.saturating_to(),
            gas_priority_fee: max_priority_fee_per_gas.map(|v| v.saturating_to()),
            kind: to.unwrap_or(TxKind::Create),
            value: value.unwrap_or_default(),
            data: input,
            chain_id: Some(chain_id),
            access_list: access_list.unwrap_or_default(),
            // EIP-4844 fields
            blob_hashes: blob_versioned_hashes.unwrap_or_default(),
            max_fee_per_blob_gas: max_fee_per_blob_gas
                .map(|v| v.saturating_to())
                .unwrap_or_default(),
            // EIP-7702 fields
            authorization_list: authorization_list
                .unwrap_or_default()
                .into_iter()
                .map(Either::Left)
                .collect(),
            frame_transaction: None,
        };

        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{vec, vec::Vec};
    use alloy_eips::eip8141::{Frame, FrameSignature};
    use alloy_primitives::{Address, TxKind};

    #[test]
    fn frame_request_rejects_outer_transaction_fields() {
        let request = TransactionRequest {
            to: Some(TxKind::Call(Address::ZERO)),
            frames: Some(Vec::new()),
            ..Default::default()
        };
        let evm_env: EvmEnv = EvmEnv::default();

        assert!(matches!(
            request.try_into_tx_env(&evm_env),
            Err(EthTxEnvError::Eip8141InvalidOuterFields)
        ));
    }

    #[test]
    fn frame_request_without_blobs_converts() {
        let request = TransactionRequest {
            from: Some(Address::ZERO),
            nonce: Some(0),
            max_fee: Some(U256::from(1)),
            max_priority_fee_per_gas: Some(0),
            blob_versioned_hashes: Some(Vec::new()),
            frames: Some(vec![Frame::default().into()]),
            signatures: Some(Vec::new()),
            transaction_type: Some(TxType::Eip8141 as u8),
            ..Default::default()
        };
        let evm_env: EvmEnv = EvmEnv::default();

        let env = request.try_into_tx_env(&evm_env).expect("valid frame request");
        assert_eq!(env.tx_type, TxType::Eip8141 as u8);
        assert!(env.blob_hashes.is_empty());
        assert_eq!(env.frame_transaction.as_deref().unwrap().max_fee, U256::from(1));
    }

    #[test]
    fn frame_request_preserves_full_width_fees() {
        let fees = TransactionFees {
            max_priority_fee_per_gas: U256::from(u128::MAX) + U256::from(1),
            max_fee: U256::from(u128::MAX) + U256::from(2),
        };
        let request = TransactionRequest {
            from: Some(Address::ZERO),
            nonce: Some(0),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            blob_versioned_hashes: Some(Vec::new()),
            frames: Some(vec![Frame::default().into()]),
            signatures: Some(Vec::<FrameSignature>::new()),
            eip8141_fees: Some(fees),
            transaction_type: Some(TxType::Eip8141 as u8),
            ..Default::default()
        };
        let evm_env: EvmEnv = EvmEnv::default();

        let env = request.try_into_tx_env(&evm_env).expect("valid frame request");
        let frame_tx = env.frame_transaction.as_deref().expect("frame payload");
        assert_eq!(frame_tx.max_priority_fee_per_gas, fees.max_priority_fee_per_gas);
        assert_eq!(frame_tx.max_fee, fees.max_fee);
    }

    #[test]
    fn frame_request_explicit_fees_override_full_width_fallback() {
        let request = TransactionRequest {
            frames: Some(vec![Frame::default().into()]),
            max_fee: Some(U256::from(20)),
            max_priority_fee_per_gas: Some(2),
            eip8141_fees: Some(TransactionFees {
                max_fee: U256::from(u128::MAX) + U256::from(1),
                max_priority_fee_per_gas: U256::from(10),
            }),
            ..Default::default()
        };
        let evm_env: EvmEnv = EvmEnv::default();
        let env = request.try_into_tx_env(&evm_env).expect("valid overridden fees");
        let fees = env.frame_transaction.as_deref().unwrap();
        assert_eq!(fees.max_fee, U256::from(20));
        assert_eq!(fees.max_priority_fee_per_gas, U256::from(2));
    }

    #[test]
    fn frame_request_rejects_per_resource_fee_caps() {
        let request = TransactionRequest {
            frames: Some(vec![Frame::default().into()]),
            max_fee_per_gas: Some(20),
            ..Default::default()
        };
        let env: EvmEnv = EvmEnv::default();
        assert!(matches!(
            request.try_into_tx_env(&env),
            Err(EthTxEnvError::Eip8141InvalidOuterFields)
        ));
    }
}
