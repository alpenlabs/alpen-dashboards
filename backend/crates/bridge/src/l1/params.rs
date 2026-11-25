//! Historical covenant selection from the bridge's protocol-parameter file.

use std::{collections::BTreeSet, fs, io, path::Path};

use bitcoin::{
    hashes::{sha256, Hash},
    secp256k1::{Parity, PublicKey},
    Network,
};
use musig2::KeyAggContext;
use strata_bridge_common::params::Params;
use strata_l1_txfmt::MagicBytes;

use super::deposits::{DepositError, DepositParams, DepositParser};

#[derive(Debug, thiserror::Error)]
pub enum BitcoinParamsError {
    #[error("cannot read Bitcoin protocol parameters: {0}")]
    Read(#[from] io::Error),
    #[error("invalid bridge protocol parameters: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot serialize Bitcoin protocol parameters: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("cannot aggregate covenant keys at height {0}")]
    Covenant(u64),
    #[error(transparent)]
    Deposit(#[from] DepositError),
}

struct CovenantPeriod {
    start_height: u64,
    deposit_params: Option<DepositParams>,
}

/// Validated deposit parameters for every operator-set activation interval.
/// Requests keep the covenant selected at their inclusion height when spent later.
pub struct BitcoinParams {
    pub(crate) network: Network,
    pub(crate) genesis_height: u64,
    pub(crate) recovery_delay: u16,
    pub(crate) params_hash: sha256::Hash,
    covenant_periods: Vec<CovenantPeriod>,
}

impl BitcoinParams {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, BitcoinParamsError> {
        Self::from_toml(&fs::read_to_string(path)?)
    }

    fn from_toml(input: &str) -> Result<Self, BitcoinParamsError> {
        let params: Params = toml::from_str(input)?;
        let params_hash = sha256::Hash::hash(toml::to_string(&params)?.as_bytes());
        let heights = params
            .keys
            .operators
            .iter()
            .flat_map(|op| {
                [Some(op.activation_height()), op.deactivation_height()]
                    .into_iter()
                    .flatten()
            })
            .collect::<BTreeSet<_>>();
        let mut covenant_periods = Vec::with_capacity(heights.len());
        for height in heights {
            let keys = params
                .keys
                .operators
                .active_at(height)
                .map(|op| PublicKey::from_x_only_public_key(op.covenant_key(), Parity::Even))
                .collect::<Vec<_>>();
            let deposit = if keys.is_empty() {
                None
            } else {
                let context =
                    KeyAggContext::new(keys).map_err(|_| BitcoinParamsError::Covenant(height))?;
                let aggregate: PublicKey = context.aggregated_pubkey();
                Some(DepositParams::new(
                    MagicBytes::new(params.protocol.magic_bytes.into_inner()),
                    aggregate.x_only_public_key().0,
                    params.protocol.deposit_amount,
                    params.protocol.recovery_delay,
                )?)
            };
            covenant_periods.push(CovenantPeriod {
                start_height: height,
                deposit_params: deposit,
            });
        }
        Ok(Self {
            network: params.network,
            genesis_height: params.genesis_height,
            recovery_delay: params.protocol.recovery_delay,
            params_hash,
            covenant_periods,
        })
    }

    pub(crate) fn parser_at(&self, height: u64) -> Option<DepositParser> {
        self.covenant_periods
            .iter()
            .rev()
            .find(|period| period.start_height <= height)
            .and_then(|period| period.deposit_params.clone())
            .map(DepositParser::new)
    }

    /// Provides network-level DT discovery independently of the current operator set.
    pub(crate) fn discovery_parser(&self) -> Option<DepositParser> {
        self.covenant_periods
            .iter()
            .find_map(|period| period.deposit_params.clone())
            .map(DepositParser::new)
    }

    #[cfg(test)]
    pub(super) fn with_test_covenant_period(mut self, height: u64, params: DepositParams) -> Self {
        self.covenant_periods.push(CovenantPeriod {
            start_height: height,
            deposit_params: Some(params),
        });
        self.covenant_periods
            .sort_by_key(|period| period.start_height);
        self
    }

    #[cfg(test)]
    pub(crate) fn for_test(network: Network, genesis_height: u64, params: DepositParams) -> Self {
        Self {
            network,
            genesis_height,
            recovery_delay: params.recovery_delay(),
            params_hash: sha256::Hash::hash(b"observer fixture"),
            covenant_periods: vec![CovenantPeriod {
                start_height: 0,
                deposit_params: Some(params),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::{
        secp256k1::{Parity, PublicKey},
        Network,
    };
    use musig2::KeyAggContext;
    use strata_asm_proto_bridge_v1_txs::deposit_request::{
        create_deposit_request_locking_script, parse_drt,
    };

    use super::BitcoinParams;
    use crate::l1::test_utils::{make_keypair, DepositFixture};

    fn make_params_toml() -> String {
        // Public keys are deterministic fixture keys; the P2P key is the Ed25519 base point.
        let first = make_keypair(1).x_only_public_key().0;
        let second = make_keypair(4).x_only_public_key().0;
        format!(
            r#"
network = "regtest"
genesis_height = 1
[keys.admin]
pubkeys = ["{first}"]
threshold = 1
[[keys.operators]]
index = 0
covenant_key = "{first}"
p2p_key = "5866666666666666666666666666666666666666666666666666666666666666"
payout_descriptor = "04{first}"
activation_height = 1
deactivation_height = 3
[[keys.operators]]
index = 1
covenant_key = "{second}"
p2p_key = "0de7729dcbeb5069136ee4bff1c4f2fd822fe8fbc9b518df434d4f0c6312d8f5"
payout_descriptor = "04{second}"
activation_height = 3
[protocol]
bury_depth = 2
magic_bytes = "ALPN"
deposit_amount = 100000000
stake_amount = 10000000
operator_fee = 1000
sweep_fee_rate = 10
recovery_delay = 144
contest_timelock = 144
proof_timelock = 144
ack_timelock = 432
nack_timelock = 144
contested_payout_timelock = 1008
unstaking_timelock = 2016
"#
        )
    }

    #[test]
    fn bitcoin_params_parse_the_committed_regtest_file() {
        let params =
            BitcoinParams::from_toml(include_str!("../../../../bridge-params.regtest.toml"))
                .expect("development bridge parameters");
        assert_eq!(params.network, Network::Regtest);
        assert!(params.parser_at(params.genesis_height).is_some());
    }

    #[test]
    fn bitcoin_params_select_the_covenant_active_at_the_request_height() {
        let params =
            BitcoinParams::from_toml(&make_params_toml()).expect("shared bridge parameters");
        assert_eq!(params.network, Network::Regtest);
        assert_eq!(params.genesis_height, 1);
        assert!(params.parser_at(0).is_none());
        let fixture = DepositFixture::new();
        let mut request = fixture.request_tx().clone();
        let context = KeyAggContext::new([PublicKey::from_x_only_public_key(
            fixture.params().covenant_key(),
            Parity::Even,
        )])
        .expect("aggregate key");
        let aggregate: PublicKey = context.aggregated_pubkey();
        request.output[1].script_pubkey = create_deposit_request_locking_script(
            parse_drt(fixture.request_tx())
                .expect("request structure")
                .header_aux()
                .recovery_pk(),
            aggregate.x_only_public_key().0,
            fixture.params().recovery_delay(),
        );
        assert!(params
            .parser_at(1)
            .expect("first covenant")
            .parse_deposit_request(&request)
            .expect("request accepted")
            .is_some());
        assert!(params
            .parser_at(2)
            .expect("first covenant still active")
            .parse_deposit_request(&request)
            .expect("request accepted")
            .is_some());
        assert!(params
            .parser_at(3)
            .expect("replacement covenant")
            .parse_deposit_request(&request)
            .is_err());
    }

    #[test]
    fn bitcoin_params_ignore_source_formatting_when_binding_stored_observations() {
        let original = make_params_toml();
        let first = BitcoinParams::from_toml(&original).expect("parameters");
        let formatted =
            BitcoinParams::from_toml(&format!("# comment\n{original}\n")).expect("same parameters");
        assert_eq!(first.params_hash, formatted.params_hash);
        let changed = BitcoinParams::from_toml(
            &original.replace("recovery_delay = 144", "recovery_delay = 145"),
        )
        .expect("changed parameters");
        assert_ne!(first.params_hash, changed.params_hash);
    }
}
