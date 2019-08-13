use failure::ResultExt;
use parity_scale_codec::Encode;
use secp256k1::schnorrsig::SchnorrSignature;
use secstr::SecUtf8;

use chain_core::common::{Proof, H256};
use chain_core::init::address::RedeemAddress;
use chain_core::init::coin::{sum_coins, Coin};
use chain_core::state::account::StakedStateAddress;
use chain_core::tx::data::address::ExtendedAddr;
use chain_core::tx::data::attribute::TxAttributes;
use chain_core::tx::data::input::TxoPointer;
use chain_core::tx::data::output::TxOut;
use chain_core::tx::witness::tree::RawPubkey;
use chain_core::tx::TxAux;
use client_common::balance::TransactionChange;
use client_common::storage::UnauthorizedStorage;
use client_common::{Error, ErrorKind, PrivateKey, PublicKey, Result, Storage};
use client_index::index::{Index, UnauthorizedIndex};

use crate::service::*;
use crate::transaction_builder::UnauthorizedTransactionBuilder;
use crate::{
    InputSelectionStrategy, MultiSigWalletClient, TransactionBuilder, UnspentTransactions,
    WalletClient,
};

/// Default implementation of `WalletClient` based on `Storage` and `Index`
#[derive(Debug, Default, Clone)]
pub struct DefaultWalletClient<S, I, T>
where
    S: Storage,
    I: Index,
    T: TransactionBuilder,
{
    key_service: KeyService<S>,
    wallet_service: WalletService<S>,
    root_hash_service: RootHashService<S>,
    multi_sig_session_service: MultiSigSessionService<S>,
    index: I,
    transaction_builder: T,
}

impl<S, I, T> DefaultWalletClient<S, I, T>
where
    S: Storage + Clone,
    I: Index,
    T: TransactionBuilder,
{
    /// Creates a new instance of `DefaultWalletClient`
    fn new(storage: S, index: I, transaction_builder: T) -> Self {
        Self {
            key_service: KeyService::new(storage.clone()),
            wallet_service: WalletService::new(storage.clone()),
            root_hash_service: RootHashService::new(storage.clone()),
            multi_sig_session_service: MultiSigSessionService::new(storage),
            index,
            transaction_builder,
        }
    }
}

impl DefaultWalletClient<UnauthorizedStorage, UnauthorizedIndex, UnauthorizedTransactionBuilder> {
    /// Returns builder for `DefaultWalletClient`
    pub fn builder() -> DefaultWalletClientBuilder<
        UnauthorizedStorage,
        UnauthorizedIndex,
        UnauthorizedTransactionBuilder,
    > {
        DefaultWalletClientBuilder::default()
    }
}

impl<S, I, T> WalletClient for DefaultWalletClient<S, I, T>
where
    S: Storage,
    I: Index,
    T: TransactionBuilder,
{
    #[inline]
    fn wallets(&self) -> Result<Vec<String>> {
        self.wallet_service.names()
    }

    fn new_wallet(&self, name: &str, passphrase: &SecUtf8) -> Result<()> {
        let view_key = self.key_service.generate_keypair(passphrase)?.0;
        self.wallet_service.create(name, passphrase, view_key)
    }

    #[inline]
    fn view_key(&self, name: &str, passphrase: &SecUtf8) -> Result<PublicKey> {
        self.wallet_service.view_key(name, passphrase)
    }

    #[inline]
    fn public_keys(&self, name: &str, passphrase: &SecUtf8) -> Result<Vec<PublicKey>> {
        self.wallet_service.public_keys(name, passphrase)
    }

    #[inline]
    fn root_hashes(&self, name: &str, passphrase: &SecUtf8) -> Result<Vec<H256>> {
        self.wallet_service.root_hashes(name, passphrase)
    }

    #[inline]
    fn staking_addresses(
        &self,
        name: &str,
        passphrase: &SecUtf8,
    ) -> Result<Vec<StakedStateAddress>> {
        self.wallet_service.staking_addresses(name, passphrase)
    }

    #[inline]
    fn transfer_addresses(&self, name: &str, passphrase: &SecUtf8) -> Result<Vec<ExtendedAddr>> {
        self.wallet_service.transfer_addresses(name, passphrase)
    }

    #[inline]
    fn find_public_key(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        redeem_address: &RedeemAddress,
    ) -> Result<Option<PublicKey>> {
        self.wallet_service
            .find_public_key(name, passphrase, redeem_address)
    }

    #[inline]
    fn find_root_hash(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        address: &ExtendedAddr,
    ) -> Result<Option<H256>> {
        self.wallet_service
            .find_root_hash(name, passphrase, address)
    }

    #[inline]
    fn private_key(
        &self,
        passphrase: &SecUtf8,
        public_key: &PublicKey,
    ) -> Result<Option<PrivateKey>> {
        self.key_service.private_key(public_key, passphrase)
    }

    fn new_public_key(&self, name: &str, passphrase: &SecUtf8) -> Result<PublicKey> {
        let (public_key, _) = self.key_service.generate_keypair(passphrase)?;
        self.wallet_service
            .add_public_key(name, passphrase, &public_key)?;

        Ok(public_key)
    }

    fn new_staking_address(&self, name: &str, passphrase: &SecUtf8) -> Result<StakedStateAddress> {
        let public_key = self.new_public_key(name, passphrase)?;
        Ok(StakedStateAddress::BasicRedeem(RedeemAddress::from(
            &public_key,
        )))
    }

    fn new_transfer_address(&self, name: &str, passphrase: &SecUtf8) -> Result<ExtendedAddr> {
        let public_key = self.new_public_key(name, passphrase)?;
        self.new_multisig_transfer_address(
            name,
            passphrase,
            vec![public_key.clone()],
            public_key,
            1,
            1,
        )
    }

    fn new_multisig_transfer_address(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        public_keys: Vec<PublicKey>,
        self_public_key: PublicKey,
        m: usize,
        n: usize,
    ) -> Result<ExtendedAddr> {
        // To verify if the passphrase is correct or not
        self.transfer_addresses(name, passphrase)?;

        let root_hash =
            self.root_hash_service
                .new_root_hash(public_keys, self_public_key, m, n, passphrase)?;

        self.wallet_service
            .add_root_hash(name, passphrase, root_hash)?;

        Ok(ExtendedAddr::OrTree(root_hash))
    }

    fn generate_proof(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        address: &ExtendedAddr,
        public_keys: Vec<PublicKey>,
    ) -> Result<Proof<RawPubkey>> {
        // To verify if the passphrase is correct or not
        self.transfer_addresses(name, passphrase)?;

        match address {
            ExtendedAddr::OrTree(ref address) => {
                self.root_hash_service
                    .generate_proof(address, public_keys, passphrase)
            }
        }
    }

    fn required_cosigners(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        root_hash: &H256,
    ) -> Result<usize> {
        // To verify if the passphrase is correct or not
        self.transfer_addresses(name, passphrase)?;

        self.root_hash_service
            .required_signers(root_hash, passphrase)
    }

    fn balance(&self, name: &str, passphrase: &SecUtf8) -> Result<Coin> {
        let addresses = self.transfer_addresses(name, passphrase)?;

        let balances = addresses
            .iter()
            .map(|address| Ok(self.index.address_details(address)?.balance))
            .collect::<Result<Vec<Coin>>>()?;

        Ok(sum_coins(balances.into_iter()).context(ErrorKind::BalanceAdditionError)?)
    }

    fn history(&self, name: &str, passphrase: &SecUtf8) -> Result<Vec<TransactionChange>> {
        let addresses = self.transfer_addresses(name, passphrase)?;

        let history = addresses
            .iter()
            .map(|address| Ok(self.index.address_details(address)?.transaction_history))
            .collect::<Result<Vec<Vec<TransactionChange>>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<TransactionChange>>();

        Ok(history)
    }

    fn unspent_transactions(
        &self,
        name: &str,
        passphrase: &SecUtf8,
    ) -> Result<UnspentTransactions> {
        let addresses = self.transfer_addresses(name, passphrase)?;

        let mut unspent_transactions = Vec::new();
        for address in addresses {
            unspent_transactions.extend(self.index.address_details(&address)?.unspent_transactions);
        }

        Ok(UnspentTransactions::new(unspent_transactions))
    }

    #[inline]
    fn output(&self, input: &TxoPointer) -> Result<TxOut> {
        self.index.output(input)
    }

    fn create_transaction(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        outputs: Vec<TxOut>,
        attributes: TxAttributes,
        input_selection_strategy: Option<InputSelectionStrategy>,
        return_address: ExtendedAddr,
    ) -> Result<TxAux> {
        let mut unspent_transactions = self.unspent_transactions(name, passphrase)?;
        unspent_transactions.apply_all(input_selection_strategy.unwrap_or_default().as_ref());

        self.transaction_builder.build(
            name,
            passphrase,
            outputs,
            attributes,
            unspent_transactions,
            return_address,
        )
    }

    #[inline]
    fn broadcast_transaction(&self, tx_aux: &TxAux) -> Result<()> {
        self.index.broadcast_transaction(&tx_aux.encode())
    }
}

impl<S, I, T> MultiSigWalletClient for DefaultWalletClient<S, I, T>
where
    S: Storage,
    I: Index,
    T: TransactionBuilder,
{
    fn schnorr_signature(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        message: &H256,
        public_key: &PublicKey,
    ) -> Result<SchnorrSignature> {
        // To verify if the passphrase is correct or not
        self.transfer_addresses(name, passphrase)?;

        let private_key = self
            .private_key(passphrase, public_key)?
            .ok_or_else(|| Error::from(ErrorKind::PrivateKeyNotFound))?;
        private_key.schnorr_sign(message)
    }

    fn new_multi_sig_session(
        &self,
        name: &str,
        passphrase: &SecUtf8,
        message: H256,
        signer_public_keys: Vec<PublicKey>,
        self_public_key: PublicKey,
    ) -> Result<H256> {
        // To verify if the passphrase is correct or not
        self.transfer_addresses(name, passphrase)?;

        let self_private_key = self
            .private_key(passphrase, &self_public_key)?
            .ok_or_else(|| Error::from(ErrorKind::PrivateKeyNotFound))?;

        self.multi_sig_session_service.new_session(
            message,
            signer_public_keys,
            self_public_key,
            self_private_key,
            passphrase,
        )
    }

    fn nonce_commitment(&self, session_id: &H256, passphrase: &SecUtf8) -> Result<H256> {
        self.multi_sig_session_service
            .nonce_commitment(session_id, passphrase)
    }

    fn add_nonce_commitment(
        &self,
        session_id: &H256,
        passphrase: &SecUtf8,
        nonce_commitment: H256,
        public_key: &PublicKey,
    ) -> Result<()> {
        self.multi_sig_session_service.add_nonce_commitment(
            session_id,
            nonce_commitment,
            public_key,
            passphrase,
        )
    }

    fn nonce(&self, session_id: &H256, passphrase: &SecUtf8) -> Result<PublicKey> {
        self.multi_sig_session_service.nonce(session_id, passphrase)
    }

    fn add_nonce(
        &self,
        session_id: &H256,
        passphrase: &SecUtf8,
        nonce: &PublicKey,
        public_key: &PublicKey,
    ) -> Result<()> {
        self.multi_sig_session_service
            .add_nonce(session_id, &nonce, public_key, passphrase)
    }

    fn partial_signature(&self, session_id: &H256, passphrase: &SecUtf8) -> Result<H256> {
        self.multi_sig_session_service
            .partial_signature(session_id, passphrase)
    }

    fn add_partial_signature(
        &self,
        session_id: &H256,
        passphrase: &SecUtf8,
        partial_signature: H256,
        public_key: &PublicKey,
    ) -> Result<()> {
        self.multi_sig_session_service.add_partial_signature(
            session_id,
            partial_signature,
            public_key,
            passphrase,
        )
    }

    fn signature(&self, session_id: &H256, passphrase: &SecUtf8) -> Result<SchnorrSignature> {
        self.multi_sig_session_service
            .signature(session_id, passphrase)
    }
}

#[derive(Debug)]
pub struct DefaultWalletClientBuilder<S, I, T>
where
    S: Storage + Clone,
    I: Index,
    T: TransactionBuilder,
{
    storage: S,
    index: I,
    transaction_builder: T,
    storage_set: bool,
    index_set: bool,
    transaction_builder_set: bool,
}

impl Default
    for DefaultWalletClientBuilder<
        UnauthorizedStorage,
        UnauthorizedIndex,
        UnauthorizedTransactionBuilder,
    >
{
    fn default() -> Self {
        DefaultWalletClientBuilder {
            storage: UnauthorizedStorage,
            index: UnauthorizedIndex,
            transaction_builder: UnauthorizedTransactionBuilder,
            storage_set: false,
            index_set: false,
            transaction_builder_set: false,
        }
    }
}

impl<S, I, T> DefaultWalletClientBuilder<S, I, T>
where
    S: Storage + Clone,
    I: Index,
    T: TransactionBuilder,
{
    /// Adds functionality for address generation and storage
    pub fn with_wallet<NS: Storage + Clone>(
        self,
        storage: NS,
    ) -> DefaultWalletClientBuilder<NS, I, T> {
        DefaultWalletClientBuilder {
            storage,
            index: self.index,
            transaction_builder: self.transaction_builder,
            storage_set: true,
            index_set: self.index_set,
            transaction_builder_set: self.transaction_builder_set,
        }
    }

    /// Adds functionality for balance tracking and transaction history
    pub fn with_transaction_read<NI: Index>(
        self,
        index: NI,
    ) -> DefaultWalletClientBuilder<S, NI, T> {
        DefaultWalletClientBuilder {
            storage: self.storage,
            index,
            transaction_builder: self.transaction_builder,
            storage_set: self.storage_set,
            index_set: true,
            transaction_builder_set: self.transaction_builder_set,
        }
    }

    /// Adds functionality for transaction creation and broadcasting
    pub fn with_transaction_write<NT: TransactionBuilder>(
        self,
        transaction_builder: NT,
    ) -> DefaultWalletClientBuilder<S, I, NT> {
        DefaultWalletClientBuilder {
            storage: self.storage,
            index: self.index,
            transaction_builder,
            storage_set: self.storage_set,
            index_set: self.index_set,
            transaction_builder_set: true,
        }
    }

    /// Builds `DefaultWalletClient`
    pub fn build(self) -> Result<DefaultWalletClient<S, I, T>> {
        if !self.index_set && !self.transaction_builder_set || self.storage_set && self.index_set {
            Ok(DefaultWalletClient::new(
                self.storage,
                self.index,
                self.transaction_builder,
            ))
        } else {
            Err(ErrorKind::InvalidInput.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::RwLock;
    use std::time::SystemTime;

    use chrono::DateTime;

    use chain_core::init::coin::CoinError;
    use chain_core::tx::data::input::{TxoIndex, TxoPointer};
    use chain_core::tx::data::{Tx, TxId};
    use chain_core::tx::fee::{Fee, FeeAlgorithm};
    use chain_core::tx::witness::TxInWitness;
    use chain_core::tx::{TransactionId, TxObfuscated};
    use chain_tx_validation::witness::verify_tx_address;
    use client_common::balance::BalanceChange;
    use client_common::storage::MemoryStorage;
    use client_common::{PrivateKey, SignedTransaction, Transaction};
    use client_index::{AddressDetails, TransactionObfuscation};

    use crate::signer::DefaultSigner;
    use crate::transaction_builder::DefaultTransactionBuilder;

    #[derive(Debug)]
    struct MockTransactionCipher;

    impl TransactionObfuscation for MockTransactionCipher {
        fn decrypt(
            &self,
            _transaction_ids: &[TxId],
            _private_key: &PrivateKey,
        ) -> Result<Vec<Transaction>> {
            unreachable!()
        }

        fn encrypt(&self, transaction: SignedTransaction) -> Result<TxAux> {
            let txpayload = transaction.encode();

            match transaction {
                SignedTransaction::TransferTransaction(tx, _) => Ok(TxAux::TransferTx {
                    txid: tx.id(),
                    inputs: tx.inputs.clone(),
                    no_of_outputs: tx.outputs.len() as TxoIndex,
                    payload: TxObfuscated {
                        key_from: 0,
                        nonce: [0u8; 12],
                        txpayload,
                    },
                }),
                _ => unreachable!(),
            }
        }
    }

    #[derive(Debug)]
    pub struct MockIndex {
        addr_1: ExtendedAddr,
        addr_2: ExtendedAddr,
        addr_3: ExtendedAddr,
        changed: RwLock<bool>,
    }

    impl MockIndex {
        fn new(addr_1: ExtendedAddr, addr_2: ExtendedAddr, addr_3: ExtendedAddr) -> Self {
            Self {
                addr_1,
                addr_2,
                addr_3,
                changed: RwLock::new(false),
            }
        }
    }

    impl Default for MockIndex {
        fn default() -> Self {
            Self {
                addr_1: ExtendedAddr::OrTree([0; 32]),
                addr_2: ExtendedAddr::OrTree([1; 32]),
                addr_3: ExtendedAddr::OrTree([2; 32]),
                changed: RwLock::new(false),
            }
        }
    }

    impl Index for MockIndex {
        fn address_details(&self, address: &ExtendedAddr) -> Result<AddressDetails> {
            let mut address_details = AddressDetails::default();

            if address == &self.addr_1 {
                address_details.transaction_history = vec![
                    TransactionChange {
                        transaction_id: [0u8; 32],
                        address: address.clone(),
                        balance_change: BalanceChange::Incoming(Coin::new(30).unwrap()),
                        block_height: 1,
                        block_time: DateTime::from(SystemTime::now()),
                    },
                    TransactionChange {
                        transaction_id: [1u8; 32],
                        address: address.clone(),
                        balance_change: BalanceChange::Outgoing(Coin::new(30).unwrap()),
                        block_height: 2,
                        block_time: DateTime::from(SystemTime::now()),
                    },
                ];
            } else if address == &self.addr_2 {
                if *self.changed.read().unwrap() {
                    address_details.transaction_history = vec![
                        TransactionChange {
                            transaction_id: [1u8; 32],
                            address: address.clone(),
                            balance_change: BalanceChange::Incoming(Coin::new(30).unwrap()),
                            block_height: 1,
                            block_time: DateTime::from(SystemTime::now()),
                        },
                        TransactionChange {
                            transaction_id: [2u8; 32],
                            address: address.clone(),
                            balance_change: BalanceChange::Outgoing(Coin::new(30).unwrap()),
                            block_height: 2,
                            block_time: DateTime::from(SystemTime::now()),
                        },
                    ];
                } else {
                    let mut unspent_transactions = BTreeMap::new();
                    unspent_transactions.insert(
                        TxoPointer::new([1u8; 32], 0),
                        TxOut {
                            address: self.addr_2.clone(),
                            value: Coin::new(30).unwrap(),
                            valid_from: None,
                        },
                    );

                    address_details.unspent_transactions = unspent_transactions;

                    address_details.transaction_history = vec![TransactionChange {
                        transaction_id: [1u8; 32],
                        address: address.clone(),
                        balance_change: BalanceChange::Incoming(Coin::new(30).unwrap()),
                        block_height: 2,
                        block_time: DateTime::from(SystemTime::now()),
                    }];

                    address_details.balance = Coin::new(30).unwrap();
                }
            } else if *self.changed.read().unwrap() && address == &self.addr_3 {
                let mut unspent_transactions = BTreeMap::new();
                unspent_transactions.insert(
                    TxoPointer::new([2u8; 32], 0),
                    TxOut {
                        address: self.addr_3.clone(),
                        value: Coin::new(30).unwrap(),
                        valid_from: None,
                    },
                );

                address_details.unspent_transactions = unspent_transactions;

                address_details.transaction_history = vec![TransactionChange {
                    transaction_id: [1u8; 32],
                    address: address.clone(),
                    balance_change: BalanceChange::Incoming(Coin::new(30).unwrap()),
                    block_height: 2,
                    block_time: DateTime::from(SystemTime::now()),
                }];

                address_details.balance = Coin::new(30).unwrap();
            }

            Ok(address_details)
        }

        fn transaction(&self, _: &TxId) -> Result<Option<Transaction>> {
            unreachable!();
        }

        fn output(&self, input: &TxoPointer) -> Result<TxOut> {
            let id = &input.id;
            let index = input.index;

            if id == &[0u8; 32] && index == 0 {
                Ok(TxOut {
                    address: self.addr_1.clone(),
                    value: Coin::new(30).unwrap(),
                    valid_from: None,
                })
            } else if id == &[1u8; 32] && index == 0 {
                Ok(TxOut {
                    address: self.addr_2.clone(),
                    value: Coin::new(30).unwrap(),
                    valid_from: None,
                })
            } else if *self.changed.read().unwrap() && id == &[2u8; 32] && index == 0 {
                Ok(TxOut {
                    address: self.addr_3.clone(),
                    value: Coin::new(30).unwrap(),
                    valid_from: None,
                })
            } else {
                Err(ErrorKind::TransactionNotFound.into())
            }
        }

        fn broadcast_transaction(&self, _transaction: &[u8]) -> Result<()> {
            let mut changed = self.changed.write().unwrap();
            *changed = true;
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct ZeroFeeAlgorithm;

    impl FeeAlgorithm for ZeroFeeAlgorithm {
        fn calculate_fee(&self, _num_bytes: usize) -> std::result::Result<Fee, CoinError> {
            Ok(Fee::new(Coin::zero()))
        }

        fn calculate_for_txaux(&self, _txaux: &TxAux) -> std::result::Result<Fee, CoinError> {
            Ok(Fee::new(Coin::zero()))
        }
    }

    #[test]
    fn check_wallet_flow() {
        let wallet = DefaultWalletClient::builder()
            .with_wallet(MemoryStorage::default())
            .build()
            .unwrap();

        assert!(wallet
            .transfer_addresses("name", &SecUtf8::from("passphrase"))
            .is_err());

        wallet
            .new_wallet("name", &SecUtf8::from("passphrase"))
            .expect("Unable to create a new wallet");

        assert_eq!(
            0,
            wallet
                .transfer_addresses("name", &SecUtf8::from("passphrase"))
                .unwrap()
                .len()
        );
        assert_eq!("name".to_string(), wallet.wallets().unwrap()[0]);
        assert_eq!(1, wallet.wallets().unwrap().len());

        let address = wallet
            .new_transfer_address("name", &SecUtf8::from("passphrase"))
            .expect("Unable to generate new address");

        let addresses = wallet
            .transfer_addresses("name", &SecUtf8::from("passphrase"))
            .unwrap();

        assert_eq!(1, addresses.len());
        assert_eq!(address, addresses[0], "Addresses don't match");

        assert!(wallet
            .find_root_hash("name", &SecUtf8::from("passphrase"), &address)
            .unwrap()
            .is_some());

        assert_eq!(
            ErrorKind::WalletNotFound,
            wallet
                .public_keys("name_new", &SecUtf8::from("passphrase"))
                .expect_err("Found public keys for non existent wallet")
                .kind(),
            "Invalid public key present in database"
        );

        assert_eq!(
            ErrorKind::WalletNotFound,
            wallet
                .new_public_key("name_new", &SecUtf8::from("passphrase"))
                .expect_err("Generated public key for non existent wallet")
                .kind(),
            "Error of invalid kind received"
        );
    }

    #[test]
    fn check_transaction_flow() {
        let storage = MemoryStorage::default();
        let wallet = DefaultWalletClient::builder()
            .with_wallet(storage.clone())
            .build()
            .unwrap();

        wallet
            .new_wallet("wallet_1", &SecUtf8::from("passphrase"))
            .unwrap();
        let addr_1 = wallet
            .new_transfer_address("wallet_1", &SecUtf8::from("passphrase"))
            .unwrap();
        wallet
            .new_wallet("wallet_2", &SecUtf8::from("passphrase"))
            .unwrap();
        let addr_2 = wallet
            .new_transfer_address("wallet_2", &SecUtf8::from("passphrase"))
            .unwrap();
        wallet
            .new_wallet("wallet_3", &SecUtf8::from("passphrase"))
            .unwrap();
        let addr_3 = wallet
            .new_transfer_address("wallet_3", &SecUtf8::from("passphrase"))
            .unwrap();

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .balance("wallet_1", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        let wallet = DefaultWalletClient::builder()
            .with_wallet(storage.clone())
            .with_transaction_read(MockIndex::new(
                addr_1.clone(),
                addr_2.clone(),
                addr_3.clone(),
            ))
            .build()
            .unwrap();

        assert_eq!(
            Coin::new(0).unwrap(),
            wallet
                .balance("wallet_1", &SecUtf8::from("passphrase"))
                .unwrap()
        );
        assert_eq!(
            Coin::new(30).unwrap(),
            wallet
                .balance("wallet_2", &SecUtf8::from("passphrase"))
                .unwrap()
        );
        assert_eq!(
            Coin::new(0).unwrap(),
            wallet
                .balance("wallet_3", &SecUtf8::from("passphrase"))
                .unwrap()
        );

        assert_eq!(
            2,
            wallet
                .history("wallet_1", &SecUtf8::from("passphrase"))
                .unwrap()
                .len()
        );
        assert_eq!(
            1,
            wallet
                .history("wallet_2", &SecUtf8::from("passphrase"))
                .unwrap()
                .len()
        );
        assert_eq!(
            0,
            wallet
                .history("wallet_3", &SecUtf8::from("passphrase"))
                .unwrap()
                .len()
        );

        let signer = DefaultSigner::new(storage.clone());

        let wallet = DefaultWalletClient::builder()
            .with_wallet(storage)
            .with_transaction_read(wallet.index)
            .with_transaction_write(DefaultTransactionBuilder::new(
                signer,
                ZeroFeeAlgorithm::default(),
                MockTransactionCipher,
            ))
            .build()
            .unwrap();

        let transaction = wallet
            .create_transaction(
                "wallet_2",
                &SecUtf8::from("passphrase"),
                vec![TxOut {
                    address: addr_3.clone(),
                    value: Coin::new(30).unwrap(),
                    valid_from: None,
                }],
                TxAttributes::new(171),
                None,
                addr_1.clone(),
            )
            .unwrap();

        assert!(wallet.broadcast_transaction(&transaction).is_ok());

        assert_eq!(
            Coin::new(0).unwrap(),
            wallet
                .balance("wallet_1", &SecUtf8::from("passphrase"))
                .unwrap()
        );
        assert_eq!(
            Coin::new(0).unwrap(),
            wallet
                .balance("wallet_2", &SecUtf8::from("passphrase"))
                .unwrap()
        );
        assert_eq!(
            Coin::new(30).unwrap(),
            wallet
                .balance("wallet_3", &SecUtf8::from("passphrase"))
                .unwrap()
        );

        assert_eq!(
            2,
            wallet
                .history("wallet_1", &SecUtf8::from("passphrase"))
                .unwrap()
                .len()
        );
        assert_eq!(
            2,
            wallet
                .history("wallet_2", &SecUtf8::from("passphrase"))
                .unwrap()
                .len()
        );
        assert_eq!(
            1,
            wallet
                .history("wallet_3", &SecUtf8::from("passphrase"))
                .unwrap()
                .len()
        );

        let transaction = wallet
            .create_transaction(
                "wallet_3",
                &SecUtf8::from("passphrase"),
                vec![TxOut {
                    address: addr_2.clone(),
                    value: Coin::new(20).unwrap(),
                    valid_from: None,
                }],
                TxAttributes::new(171),
                None,
                addr_1.clone(),
            )
            .unwrap();

        assert!(wallet.broadcast_transaction(&transaction).is_ok());

        assert_eq!(
            ErrorKind::InsufficientBalance,
            wallet
                .create_transaction(
                    "wallet_2",
                    &SecUtf8::from("passphrase"),
                    vec![TxOut {
                        address: addr_3.clone(),
                        value: Coin::new(30).unwrap(),
                        valid_from: None,
                    }],
                    TxAttributes::new(171),
                    None,
                    addr_1.clone()
                )
                .unwrap_err()
                .kind()
        );
    }

    #[test]
    fn check_unauthorized_wallet() {
        let wallet = DefaultWalletClient::builder().build().unwrap();

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet.wallets().unwrap_err().kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .new_wallet("name", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .public_keys("name", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .private_key(
                    &SecUtf8::from("passphrase"),
                    &PublicKey::from(&PrivateKey::new().unwrap())
                )
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .new_public_key("name", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .new_staking_address("name", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .balance("name", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .history("name", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .unspent_transactions("name", &SecUtf8::from("passphrase"))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .output(&TxoPointer::new([1u8; 32], 0))
                .unwrap_err()
                .kind()
        );

        assert_eq!(
            ErrorKind::PermissionDenied,
            wallet
                .create_transaction(
                    "name",
                    &SecUtf8::from("passphrase"),
                    Vec::new(),
                    TxAttributes::new(171),
                    None,
                    ExtendedAddr::OrTree(Default::default())
                )
                .unwrap_err()
                .kind()
        );
    }

    #[test]
    fn invalid_wallet_building() {
        let storage = MemoryStorage::default();
        let signer = DefaultSigner::new(storage);
        let builder =
            DefaultWalletClient::builder().with_transaction_write(DefaultTransactionBuilder::new(
                signer,
                ZeroFeeAlgorithm::default(),
                MockTransactionCipher,
            ));

        assert_eq!(ErrorKind::InvalidInput, builder.build().unwrap_err().kind());
    }

    #[test]
    fn check_multi_sig_address_generation() {
        let storage = MemoryStorage::default();
        let wallet = DefaultWalletClient::builder()
            .with_wallet(storage.clone())
            .build()
            .unwrap();

        let passphrase = SecUtf8::from("passphrase");
        let name = "name";

        assert_eq!(
            ErrorKind::WalletNotFound,
            wallet
                .transfer_addresses(name, &passphrase)
                .expect_err("Found non-existent addresses")
                .kind()
        );

        wallet
            .new_wallet(name, &passphrase)
            .expect("Unable to create a new wallet");

        assert_eq!(
            0,
            wallet.transfer_addresses(name, &passphrase).unwrap().len()
        );

        let public_keys = vec![
            PublicKey::from(&PrivateKey::new().unwrap()),
            PublicKey::from(&PrivateKey::new().unwrap()),
            PublicKey::from(&PrivateKey::new().unwrap()),
        ];

        let tree_address = wallet
            .new_multisig_transfer_address(
                name,
                &passphrase,
                public_keys.clone(),
                public_keys[0].clone(),
                2,
                3,
            )
            .unwrap();

        assert_eq!(
            1,
            wallet.transfer_addresses(name, &passphrase).unwrap().len()
        );

        let root_hash = wallet
            .find_root_hash(name, &passphrase, &tree_address)
            .unwrap()
            .unwrap();

        assert_eq!(
            2,
            wallet
                .required_cosigners(name, &passphrase, &root_hash)
                .unwrap()
        );
    }

    #[test]
    fn check_multi_sig_transaction_signing() {
        let storage = MemoryStorage::default();
        let wallet = DefaultWalletClient::builder()
            .with_wallet(storage.clone())
            .build()
            .unwrap();

        let passphrase = &SecUtf8::from("passphrase");
        let name = "name";

        wallet.new_wallet(name, passphrase).unwrap();

        let public_key_1 = wallet.new_public_key(name, passphrase).unwrap();
        let public_key_2 = wallet.new_public_key(name, passphrase).unwrap();
        let public_key_3 = wallet.new_public_key(name, passphrase).unwrap();

        let public_keys = vec![
            public_key_1.clone(),
            public_key_2.clone(),
            public_key_3.clone(),
        ];

        let multi_sig_address = wallet
            .new_multisig_transfer_address(
                name,
                passphrase,
                public_keys.clone(),
                public_keys[0].clone(),
                2,
                3,
            )
            .unwrap();

        let transaction = Tx::new();

        let session_id_1 = wallet
            .new_multi_sig_session(
                name,
                passphrase,
                transaction.id(),
                vec![public_key_1.clone(), public_key_2.clone()],
                public_key_1.clone(),
            )
            .unwrap();
        let session_id_2 = wallet
            .new_multi_sig_session(
                name,
                passphrase,
                transaction.id(),
                vec![public_key_1.clone(), public_key_2.clone()],
                public_key_2.clone(),
            )
            .unwrap();

        let nonce_commitment_1 = wallet.nonce_commitment(&session_id_1, passphrase).unwrap();
        let nonce_commitment_2 = wallet.nonce_commitment(&session_id_2, passphrase).unwrap();

        assert!(wallet
            .add_nonce_commitment(&session_id_1, passphrase, nonce_commitment_2, &public_key_2)
            .is_ok());
        assert!(wallet
            .add_nonce_commitment(&session_id_2, passphrase, nonce_commitment_1, &public_key_1)
            .is_ok());

        let nonce_1 = wallet.nonce(&session_id_1, passphrase).unwrap();
        let nonce_2 = wallet.nonce(&session_id_2, passphrase).unwrap();

        assert!(wallet
            .add_nonce(&session_id_1, passphrase, &nonce_2, &public_key_2)
            .is_ok());
        assert!(wallet
            .add_nonce(&session_id_2, passphrase, &nonce_1, &public_key_1)
            .is_ok());

        let partial_signature_1 = wallet.partial_signature(&session_id_1, passphrase).unwrap();
        let partial_signature_2 = wallet.partial_signature(&session_id_2, passphrase).unwrap();

        assert!(wallet
            .add_partial_signature(
                &session_id_1,
                passphrase,
                partial_signature_2,
                &public_key_2
            )
            .is_ok());
        assert!(wallet
            .add_partial_signature(
                &session_id_2,
                passphrase,
                partial_signature_1,
                &public_key_1
            )
            .is_ok());

        let signature = wallet.signature(&session_id_1, passphrase).unwrap();
        let proof = wallet
            .generate_proof(
                name,
                passphrase,
                &multi_sig_address,
                vec![public_key_1.clone(), public_key_2.clone()],
            )
            .unwrap();

        let witness = TxInWitness::TreeSig(signature, proof);

        assert!(verify_tx_address(&witness, &transaction.id(), &multi_sig_address).is_ok())
    }

    #[test]
    fn check_1_of_n_schnorr_signature() {
        let storage = MemoryStorage::default();
        let wallet = DefaultWalletClient::builder()
            .with_wallet(storage.clone())
            .build()
            .unwrap();

        let passphrase = &SecUtf8::from("passphrase");
        let name = "name";

        wallet.new_wallet(name, passphrase).unwrap();

        let public_key_1 = wallet.new_public_key(name, passphrase).unwrap();
        let public_key_2 = wallet.new_public_key(name, passphrase).unwrap();
        let public_key_3 = wallet.new_public_key(name, passphrase).unwrap();

        let public_keys = vec![
            public_key_1.clone(),
            public_key_2.clone(),
            public_key_3.clone(),
        ];

        let tree_address = wallet
            .new_multisig_transfer_address(
                name,
                passphrase,
                public_keys.clone(),
                public_keys[0].clone(),
                1,
                3,
            )
            .unwrap();

        let transaction = Tx::new();

        let signature = wallet
            .schnorr_signature(name, passphrase, &transaction.id(), &public_key_1)
            .unwrap();

        println!("Signature");

        let proof = wallet
            .generate_proof(name, passphrase, &tree_address, vec![public_key_1.clone()])
            .unwrap();

        let witness = TxInWitness::TreeSig(signature, proof);

        assert!(verify_tx_address(&witness, &transaction.id(), &tree_address).is_ok())
    }
}
