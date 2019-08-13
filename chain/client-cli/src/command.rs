mod address_command;
mod transaction_command;
mod wallet_command;

use hex::encode;
use prettytable::{cell, format, row, Cell, Row, Table};
use quest::success;
use structopt::StructOpt;

use chain_core::state::account::StakedStateAddress;
use client_common::balance::BalanceChange;
use client_common::storage::SledStorage;
use client_common::tendermint::{Client, RpcClient};
use client_common::{Error, ErrorKind, Result, Storage};
use client_core::signer::DefaultSigner;
use client_core::transaction_builder::DefaultTransactionBuilder;
use client_core::wallet::{DefaultWalletClient, WalletClient};
use client_index::cipher::MockAbciTransactionObfuscation;
use client_index::handler::{DefaultBlockHandler, DefaultTransactionHandler};
use client_index::index::DefaultIndex;
use client_index::synchronizer::ManualSynchronizer;
use client_index::BlockHandler;
use client_network::network_ops::{DefaultNetworkOpsClient, NetworkOpsClient};

use self::address_command::AddressCommand;
use self::transaction_command::TransactionCommand;
use self::wallet_command::WalletCommand;

use crate::{ask_passphrase, storage_path, tendermint_url};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "client-cli",
    about = "Basic CLI tool for interacting with Crypto.com Chain"
)]
pub enum Command {
    #[structopt(name = "wallet", about = "Wallet operations")]
    Wallet {
        #[structopt(subcommand)]
        wallet_command: WalletCommand,
    },
    #[structopt(name = "address", about = "Address operations")]
    Address {
        #[structopt(subcommand)]
        address_command: AddressCommand,
    },
    #[structopt(name = "view-key", about = "Shows the view key of a wallet")]
    ViewKey {
        #[structopt(name = "name", short, long, help = "Name of wallet")]
        name: String,
    },
    #[structopt(name = "balance", about = "Get balance of a wallet")]
    Balance {
        #[structopt(name = "name", short, long, help = "Name of wallet")]
        name: String,
    },
    #[structopt(name = "history", about = "Get transaction history of a wallet")]
    History {
        #[structopt(name = "name", short, long, help = "Name of wallet")]
        name: String,
    },
    #[structopt(name = "transaction", about = "Transaction operations")]
    Transaction {
        #[structopt(subcommand)]
        transaction_command: TransactionCommand,
    },
    #[structopt(name = "state", about = "Get staked state of an address")]
    StakedState {
        #[structopt(name = "name", short, long, help = "Name of wallet")]
        name: String,
        #[structopt(name = "address", short, long, help = "Staking address")]
        address: StakedStateAddress,
    },
    #[structopt(name = "sync", about = "Synchronize client with Crypto.com Chain")]
    Sync {
        #[structopt(name = "name", short, long, help = "Name of wallet")]
        name: String,
        #[structopt(
            name = "force",
            short,
            long,
            help = "Force synchronization from genesis"
        )]
        force: bool,
    },
}

impl Command {
    pub fn execute(&self) -> Result<()> {
        match self {
            Command::Wallet { wallet_command } => {
                let storage = SledStorage::new(storage_path())?;
                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage)
                    .build()?;
                wallet_command.execute(wallet_client)
            }
            Command::Address { address_command } => {
                let storage = SledStorage::new(storage_path())?;
                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage)
                    .build()?;
                address_command.execute(wallet_client)
            }
            Command::ViewKey { name } => {
                let storage = SledStorage::new(storage_path())?;
                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage)
                    .build()?;

                Self::get_view_key(wallet_client, name)
            }
            Command::Balance { name } => {
                let storage = SledStorage::new(storage_path())?;
                let tendermint_client = RpcClient::new(&tendermint_url());
                let transaction_index = DefaultIndex::new(storage.clone(), tendermint_client);
                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage)
                    .with_transaction_read(transaction_index)
                    .build()?;
                Self::get_balance(wallet_client, name)
            }
            Command::History { name } => {
                let storage = SledStorage::new(storage_path())?;
                let tendermint_client = RpcClient::new(&tendermint_url());
                let transaction_index = DefaultIndex::new(storage.clone(), tendermint_client);
                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage)
                    .with_transaction_read(transaction_index)
                    .build()?;
                Self::get_history(wallet_client, name)
            }
            Command::Transaction {
                transaction_command,
            } => {
                let storage = SledStorage::new(storage_path())?;
                let tendermint_client = RpcClient::new(&tendermint_url());
                let signer = DefaultSigner::new(storage.clone());
                let fee_algorithm = tendermint_client.genesis()?.fee_policy();
                let transaction_cipher =
                    MockAbciTransactionObfuscation::new(tendermint_client.clone());
                let transaction_builder = DefaultTransactionBuilder::new(
                    signer.clone(),
                    fee_algorithm,
                    transaction_cipher.clone(),
                );
                let transaction_index =
                    DefaultIndex::new(storage.clone(), tendermint_client.clone());

                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage.clone())
                    .with_transaction_read(transaction_index)
                    .with_transaction_write(transaction_builder)
                    .build()?;
                let network_ops_client = DefaultNetworkOpsClient::new(
                    wallet_client,
                    signer,
                    tendermint_client,
                    fee_algorithm,
                    transaction_cipher,
                );
                transaction_command.execute(network_ops_client.get_wallet(), &network_ops_client)
            }
            Command::StakedState { name, address } => {
                let storage = SledStorage::new(storage_path())?;
                let tendermint_client = RpcClient::new(&tendermint_url());
                let signer = DefaultSigner::new(storage.clone());
                let fee_algorithm = tendermint_client.genesis()?.fee_policy();
                let transaction_cipher =
                    MockAbciTransactionObfuscation::new(tendermint_client.clone());
                let transaction_builder = DefaultTransactionBuilder::new(
                    signer.clone(),
                    fee_algorithm,
                    transaction_cipher.clone(),
                );
                let transaction_index =
                    DefaultIndex::new(storage.clone(), tendermint_client.clone());
                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage)
                    .with_transaction_read(transaction_index)
                    .with_transaction_write(transaction_builder)
                    .build()?;

                let network_ops_client = DefaultNetworkOpsClient::new(
                    wallet_client,
                    signer,
                    tendermint_client,
                    fee_algorithm,
                    transaction_cipher,
                );
                Self::get_staked_stake(&network_ops_client, name, address)
            }
            Command::Sync { name, force } => {
                let storage = SledStorage::new(storage_path())?;
                let tendermint_client = RpcClient::new(&tendermint_url());

                let transaction_handler = DefaultTransactionHandler::new(storage.clone());
                let transaction_cipher =
                    MockAbciTransactionObfuscation::new(tendermint_client.clone());
                let block_handler = DefaultBlockHandler::new(
                    transaction_cipher,
                    transaction_handler,
                    storage.clone(),
                );

                let wallet_client = DefaultWalletClient::builder()
                    .with_wallet(storage.clone())
                    .build()?;
                let synchronizer =
                    ManualSynchronizer::new(storage, tendermint_client, block_handler);

                Self::resync(wallet_client, synchronizer, name, *force)
            }
        }
    }

    fn get_staked_stake<N: NetworkOpsClient>(
        network_ops_client: &N,
        name: &str,
        address: &StakedStateAddress,
    ) -> Result<()> {
        let passphrase = ask_passphrase(None)?;
        let staked_state = network_ops_client.get_staked_state(name, &passphrase, address)?;

        let mut table = Table::new();
        table.set_format(*format::consts::FORMAT_NO_LINESEP);

        table.set_titles(row!["Nonce", format!("{}", staked_state.nonce)]);
        table.add_row(Row::new(vec![
            Cell::from(&"Bonded".to_string()),
            Cell::from(&format!("{}", staked_state.bonded)),
        ]));
        table.add_row(Row::new(vec![
            Cell::from(&"Unbonded".to_string()),
            Cell::from(&format!("{}", staked_state.unbonded)),
        ]));
        table.add_row(Row::new(vec![
            Cell::from(&"Unbonded From".to_string()),
            Cell::from(&format!("{}", staked_state.unbonded_from)),
        ]));

        table.printstd();

        Ok(())
    }

    fn get_view_key<T: WalletClient>(wallet_client: T, name: &str) -> Result<()> {
        let passphrase = ask_passphrase(None)?;
        let view_key = wallet_client.view_key(name, &passphrase)?;

        success(&format!("View Key: {}", view_key));
        Ok(())
    }

    fn get_balance<T: WalletClient>(wallet_client: T, name: &str) -> Result<()> {
        let passphrase = ask_passphrase(None)?;
        let balance = wallet_client.balance(name, &passphrase)?;

        success(&format!("Wallet balance: {}", balance));
        Ok(())
    }

    fn get_history<T: WalletClient>(wallet_client: T, name: &str) -> Result<()> {
        let passphrase = ask_passphrase(None)?;
        let history = wallet_client.history(name, &passphrase)?;

        if !history.is_empty() {
            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
            table.set_titles(row![
                "Transaction ID",
                "Address",
                "Amount",
                "In/Out",
                "Block Height",
                "Block Time",
            ]);

            for change in history {
                let (amount, in_out, spec) = match change.balance_change {
                    BalanceChange::Incoming(amount) => (amount, "IN", "Fg"),
                    BalanceChange::Outgoing(amount) => (amount, "OUT", "FR"),
                };

                table.add_row(Row::new(vec![
                    Cell::new(&encode(&change.transaction_id)),
                    Cell::from(&change.address),
                    Cell::from(&amount).style_spec("r"),
                    Cell::new(in_out).style_spec(spec),
                    Cell::from(&change.block_height).style_spec("r"),
                    Cell::from(&change.block_time),
                ]));
            }

            table.printstd();
        } else {
            success("No history found!")
        }

        Ok(())
    }

    fn resync<T: WalletClient, S: Storage, C: Client, H: BlockHandler>(
        wallet_client: T,
        synchronizer: ManualSynchronizer<S, C, H>,
        name: &str,
        force: bool,
    ) -> Result<()> {
        let passphrase = ask_passphrase(None)?;

        let view_key = wallet_client.view_key(name, &passphrase)?;
        let private_key = wallet_client
            .private_key(&passphrase, &view_key)?
            .ok_or_else(|| Error::from(ErrorKind::WalletNotFound))?;

        let staking_addresses = wallet_client.staking_addresses(name, &passphrase)?;

        if force {
            synchronizer.sync_all(&staking_addresses, &view_key, &private_key)
        } else {
            synchronizer.sync(&staking_addresses, &view_key, &private_key)
        }
    }
}
