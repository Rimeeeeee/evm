//! EIP-8253 irregular nonce transition.
//!
//! Spec and Mainnet list: ethereum/EIPs@fb527daa1830467d6be1336cbf36ed11ef23e6a2.

use super::spec::EthExecutorSpec;
use crate::{block::BlockExecutionError, Evm};
use alloy_hardforks::{EthereumHardfork, ForkCondition};
use alloy_primitives::{address, Address};
use revm::{context::Block, state::Account, Database, DatabaseCommit};

/// Mainnet accounts with zero nonce, empty code, and non-empty storage before the fork.
/// Other chains must configure their own list, even when they share Mainnet's chain ID.
pub const MAINNET_ACCOUNTS: [Address; 28] = [
    address!("f468bcbc4a0bfdb06336e773382c5202e674db71"),
    address!("d8253352f6044cfe55bcc0748c3fa37b7df81f98"),
    address!("5983c6ac846dcf85fbbc4303f43eb91c379f79ae"),
    address!("de425ad4b8d2d9e0e12f65cbcd6d55f447b44083"),
    address!("50b1497068bae652df3562eb8ea7677ff84477fa"),
    address!("8398ff6c618e9515468c1c4b198d53666cbe8462"),
    address!("6f156dbf8ed30e53f7c9df73144e69f65cbb7e94"),
    address!("2c081ed1949d7dd9447f9d96e509befe576d4461"),
    address!("db7c577b93baeb56dab50af4d6f86f99a06b96a2"),
    address!("14725085d004f1b10ee07234a4ab28c5ad2a7b9e"),
    address!("ae3703584494ade958ad27ec2d289b7a67c19e90"),
    address!("7d6ae067de8d44ae1a08750e7d626d61a623c44a"),
    address!("4d149eb99bdeefc1f858f8fd22289c6beae99f2c"),
    address!("361d7a60b43587c7f6bba4f9fd9642747f65210a"),
    address!("b619f45637c39ca49a41ac64c11637a0a194455e"),
    address!("5071cb62aa170b7f66b26cae8004d90e6078bb1e"),
    address!("add92e0650457c5db0c4c08cbf7ca580175d33d2"),
    address!("3311c08066580cb906a7287b6786e504c2ebd09f"),
    address!("02820e4bee488c40f7455fdca53125565148708f"),
    address!("e62dc49c92fa799033644d2a9afd7e3babe5a80a"),
    address!("5cc182fabfb81a056b6080d4200bc5150673d06f"),
    address!("f4a835ec1364809003de3925685f24cd360bdffe"),
    address!("fc4465f84b29a1f8794dc753f41bef1f4b025ed2"),
    address!("40490c9c468622d5c89646d6f3097f8eaf80c411"),
    address!("a21b22389bfc1cd6bc7ba19a4fc96adc3d0fe074"),
    address!("59ec0410867828e3b8c23dd8a29d9796ef523b17"),
    address!("19272418753b90d9a3e3efc8430b1612c55fcb3a"),
    address!("fee7707fa4b8c0a923a0e40399db3e7ce26069c6"),
];

/// Sets the configured nonces to one on the Bogota fork block, before system calls.
pub fn apply<E: Evm<DB: DatabaseCommit>>(
    evm: &mut E,
    spec: &impl EthExecutorSpec,
    parent_timestamp: Option<u64>,
) -> Result<(), BlockExecutionError> {
    let accounts = spec.eip8253_accounts();
    if accounts.is_empty() {
        return Ok(());
    }
    let number: u64 = evm.block().number().saturating_to();
    let timestamp: u64 = evm.block().timestamp().saturating_to();
    let transition = match spec.ethereum_fork_activation(EthereumHardfork::Bogota) {
        ForkCondition::Block(block) => number != 0 && number == block,
        ForkCondition::Timestamp(at) if at != 0 && timestamp >= at && number != 0 => {
            let parent = match parent_timestamp {
                Some(timestamp) => timestamp,
                None => evm
                    .db_mut()
                    .block_timestamp(number - 1)
                    .map_err(BlockExecutionError::other)?
                    .ok_or_else(|| {
                        BlockExecutionError::msg("missing parent timestamp for EIP-8253 transition")
                    })?,
            };
            parent < at
        }
        _ => false,
    };
    if !transition {
        return Ok(());
    }
    for address in accounts {
        let info =
            evm.db_mut().basic(*address).map_err(BlockExecutionError::other)?.unwrap_or_default();
        let mut account = Account::from(info);
        account.info.nonce = 1;
        account.mark_touch();
        evm.db_mut().commit_iter(&mut core::iter::once((*address, account)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eth::EthEvmFactory, EvmEnv, EvmFactory};
    use alloy_hardforks::EthereumHardforks;
    use alloy_primitives::U256;
    use revm::{
        context::CfgEnv,
        database::{CacheDB, EmptyDB, State},
        state::AccountInfo,
    };

    struct Spec(&'static [Address]);

    impl EthereumHardforks for Spec {
        fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
            if fork == EthereumHardfork::Bogota {
                ForkCondition::Timestamp(15)
            } else {
                ForkCondition::Never
            }
        }
    }

    impl EthExecutorSpec for Spec {
        fn deposit_contract_address(&self) -> Option<Address> {
            None
        }
        fn eip8253_accounts(&self) -> &[Address] {
            self.0
        }
    }

    #[test]
    fn database_ancestry_and_unconditional_nonce_write() {
        use alloy_primitives::B256;
        use revm::{
            database_interface::{bal::BalDatabase, DatabaseRef},
            state::Bytecode,
        };
        #[derive(Debug)]
        struct Ancestor;
        impl DatabaseRef for Ancestor {
            type Error = core::convert::Infallible;
            fn basic_ref(&self, _: Address) -> Result<Option<AccountInfo>, Self::Error> {
                Ok(Some(AccountInfo::default().with_nonce(7).with_balance(U256::from(19))))
            }
            fn code_by_hash_ref(&self, _: B256) -> Result<Bytecode, Self::Error> {
                Ok(Bytecode::default())
            }
            fn storage_ref(&self, _: Address, _: U256) -> Result<U256, Self::Error> {
                Ok(U256::from(9))
            }
            fn block_hash_ref(&self, _: u64) -> Result<B256, Self::Error> {
                Ok(B256::ZERO)
            }
            fn block_timestamp_ref(&self, number: u64) -> Result<Option<u64>, Self::Error> {
                assert_eq!(number, 1);
                Ok(Some(14))
            }
        }
        let state =
            State::builder().with_database(BalDatabase::new(CacheDB::new(Ancestor))).build();
        let mut env: EvmEnv = EvmEnv::new(
            CfgEnv::new_with_spec(revm::primitives::hardfork::SpecId::BOGOTA),
            Default::default(),
        );
        env.block_env.number = U256::from(2);
        env.block_env.timestamp = U256::from(20);
        let mut evm = EthEvmFactory::default().create_evm(state, env);
        apply(&mut evm, &Spec(&MAINNET_ACCOUNTS), None).unwrap();
        let address = MAINNET_ACCOUNTS[0];
        assert_eq!(evm.db_mut().basic(address).unwrap().unwrap().nonce, 1);
        assert_eq!(evm.db_mut().basic(address).unwrap().unwrap().balance, U256::from(19));
        assert_eq!(evm.db_mut().storage(address, U256::ZERO).unwrap(), U256::from(9));
        let mut changed = Account::from(evm.db_mut().basic(address).unwrap().unwrap());
        changed.info.nonce = 3;
        changed.mark_touch();
        evm.db_mut().commit_iter(&mut core::iter::once((address, changed)));
        apply(&mut evm, &Spec(&MAINNET_ACCOUNTS), Some(20)).unwrap();
        assert_eq!(evm.db_mut().basic(address).unwrap().unwrap().nonce, 3);
    }

    #[test]
    fn nonce_bump_is_once_at_boundary_and_bal_index_zero() {
        for (parent, timestamp, applies) in
            [(13, 14, false), (14, 15, true), (14, 20, true), (15, 16, false)]
        {
            let mut db = CacheDB::<EmptyDB>::default();
            for address in MAINNET_ACCOUNTS {
                db.insert_account_info(
                    address,
                    AccountInfo::default().with_balance(U256::from(17)),
                );
                db.insert_account_storage(address, U256::from(2), U256::from(9)).unwrap();
            }
            let untouched = Address::repeat_byte(0x77);
            db.insert_account_info(untouched, AccountInfo::default().with_nonce(7));
            let state = State::builder().with_database(db).with_bal_builder().build();
            let mut env: EvmEnv = EvmEnv::new(
                CfgEnv::new_with_spec(revm::primitives::hardfork::SpecId::BOGOTA),
                Default::default(),
            );
            env.block_env.number = U256::from(2);
            env.block_env.timestamp = U256::from(timestamp);
            let mut evm = EthEvmFactory::default().create_evm(state, env);
            apply(&mut evm, &Spec(&MAINNET_ACCOUNTS), Some(parent)).unwrap();
            let state = evm.db_mut();
            for address in MAINNET_ACCOUNTS {
                let account = state.basic(address).unwrap().unwrap();
                assert_eq!(account.nonce, u64::from(applies));
                assert_eq!(account.balance, U256::from(17));
                assert_eq!(account.code_hash, alloy_primitives::KECCAK256_EMPTY);
                assert_eq!(state.storage(address, U256::from(2)).unwrap(), U256::from(9));
            }
            assert_eq!(state.basic(untouched).unwrap().unwrap().nonce, 7);
            let bal = state.take_built_alloy_bal().unwrap();
            assert_eq!(bal.len(), if applies { 28 } else { 0 });
            for account in bal {
                assert!(account.balance_changes.is_empty());
                assert!(account.storage_changes.is_empty());
                assert!(account.storage_reads.is_empty());
                assert!(account.code_changes.is_empty());
                assert_eq!(account.nonce_changes.len(), 1);
                assert_eq!(account.nonce_changes[0].block_access_index.get(), 0);
                assert_eq!(account.nonce_changes[0].new_nonce, 1);
            }
        }
    }

    #[test]
    fn empty_list_needs_no_parent_and_missing_parent_is_an_error() {
        let state = State::builder().with_database(EmptyDB::default()).with_bal_builder().build();
        let mut env: EvmEnv = EvmEnv::new(
            CfgEnv::new_with_spec(revm::primitives::hardfork::SpecId::BOGOTA),
            Default::default(),
        );
        env.block_env.number = U256::from(2);
        env.block_env.timestamp = U256::from(15);
        let mut evm = EthEvmFactory::default().create_evm(state, env);
        apply(&mut evm, &Spec(&[]), None).unwrap();
        assert!(apply(&mut evm, &Spec(&MAINNET_ACCOUNTS), None).is_err());
        assert!(evm.db_mut().take_built_alloy_bal().unwrap().is_empty());
    }
}
