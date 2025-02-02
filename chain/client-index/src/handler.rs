//! Observer for block-headers, transactions, etc.
mod default_block_handler;
mod default_transaction_handler;

pub use default_block_handler::DefaultBlockHandler;
pub use default_transaction_handler::DefaultTransactionHandler;

use chrono::{DateTime, Utc};

use client_common::{BlockHeader, PrivateKey, PublicKey, Result, Transaction};

/// Interface for handling stream of transactions in Crypto.com Chain
pub trait TransactionHandler: Send + Sync {
    /// Handles a transaction on Crypto.com Chain
    fn on_next(
        &self,
        transaction: Transaction,
        block_height: u64,
        block_time: DateTime<Utc>,
    ) -> Result<()>;
}

/// Interface for handling stream of block headers in Crypto.com Chain
pub trait BlockHandler: Send + Sync {
    /// Handles a block header in Crypto.com Chain
    fn on_next(
        &self,
        block_header: BlockHeader,
        view_key: &PublicKey,
        private_key: &PrivateKey,
    ) -> Result<()>;
}
