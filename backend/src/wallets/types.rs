use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Wallet {
    /// Wallet address
    address: String,
    /// Wallet balance in Wei
    balance: String,
}

impl Wallet {
    pub(crate) fn new(address: String, balance: String) -> Self {
        Self { address, balance }
    }

    pub(crate) fn update_balance(&mut self, balance: String) {
        self.balance = balance;
    }

    pub(crate) fn address(&self) -> &str {
        &self.address
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct PaymasterWallets {
    /// Deposit paymaster wallet
    deposit: Wallet,
    /// Validating paymaster wallet
    validating: Wallet,
}

impl PaymasterWallets {
    pub(crate) fn new(deposit: Wallet, validating: Wallet) -> Self {
        Self {
            deposit,
            validating,
        }
    }

    pub(crate) fn deposit_wallet_mut(&mut self) -> &mut Wallet {
        &mut self.deposit
    }

    pub(crate) fn validating_wallet_mut(&mut self) -> &mut Wallet {
        &mut self.validating
    }
}
