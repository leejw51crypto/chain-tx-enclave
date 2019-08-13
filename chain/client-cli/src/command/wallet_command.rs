use quest::{ask, success};
use structopt::StructOpt;

use client_common::{ErrorKind, Result};
use client_core::WalletClient;

use crate::ask_passphrase;

#[derive(Debug, StructOpt)]
pub enum WalletCommand {
    #[structopt(name = "new", about = "New wallet")]
    New {
        #[structopt(name = "name", short, long, help = "Name of wallet")]
        name: String,
    },
    #[structopt(name = "list", about = "List all wallets")]
    List,
}

impl WalletCommand {
    pub fn execute<T: WalletClient>(&self, wallet_client: T) -> Result<()> {
        match self {
            WalletCommand::New { name } => Self::new_wallet(wallet_client, name),
            WalletCommand::List => Self::list_wallets(wallet_client),
        }
    }

    fn new_wallet<T: WalletClient>(wallet_client: T, name: &str) -> Result<()> {
        let passphrase = ask_passphrase(None)?;
        let confirmed_passphrase = ask_passphrase(Some("Confirm passphrase: "))?;

        if passphrase != confirmed_passphrase {
            return Err(ErrorKind::InvalidInput.into());
        }

        wallet_client.new_wallet(name, &passphrase)?;

        success(&format!("Wallet created with name: {}", name));
        Ok(())
    }

    fn list_wallets<T: WalletClient>(wallet_client: T) -> Result<()> {
        let wallets = wallet_client.wallets()?;

        for wallet in wallets {
            ask("Wallet name: ");
            success(&wallet);
        }

        Ok(())
    }
}
