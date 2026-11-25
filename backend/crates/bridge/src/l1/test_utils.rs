//! Signed deposit fixtures shared by recognition and observer tests.

use bitcoin::{
    absolute::LockTime,
    hashes::Hash,
    key::TapTweak,
    secp256k1::{Keypair, Message, SecretKey, SECP256K1},
    sighash::{Annex, Prevouts, SighashCache, TapSighashType},
    taproot::{Signature, TapLeafHash},
    transaction::Version,
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, XOnlyPublicKey,
};
use strata_asm_proto_bridge_v1_txs::{
    deposit::DepositTxHeaderAux,
    deposit_request::{
        build_deposit_request_spend_info, create_deposit_request_locking_script, DrtHeaderAux,
        DRT_OUTPUT_INDEX,
    },
};
use strata_bridge_tx_graph::transactions::deposit::DepositTx;
use strata_l1_txfmt::{MagicBytes, ParseConfig, TagData};

use super::deposits::{DepositParams, DepositParser, DepositRequest};

pub(crate) const TEST_DEPOSIT_INDEX: u32 = 7;
pub(crate) const TEST_DEPOSIT_DESTINATION: &[u8] = b"deposit destination";

pub(crate) const TEST_ANNEX: &[u8] = &[0x50, 1];
const TEST_COVENANT_KEY_SEED: u8 = 1;
const TEST_RECOVERY_KEY_SEED: u8 = 2;

pub(crate) struct DepositFixture {
    params: DepositParams,
    request_tx: Transaction,
    covenant_keypair: Keypair,
    recovery_keypair: Keypair,
}

impl DepositFixture {
    pub(crate) fn new() -> Self {
        let params = DepositParams::new(
            MagicBytes::new(*b"ALPN"),
            make_keypair(TEST_COVENANT_KEY_SEED).x_only_public_key().0,
            Amount::from_sat(100_000_000),
            144,
        )
        .expect("deposit parameters");
        Self::with_params(params)
    }

    /// Builds signed transaction fixtures using the supplied protocol parameters.
    /// Panics if the covenant key differs from the default fixture's signing key.
    pub(crate) fn with_params(params: DepositParams) -> Self {
        let covenant_keypair = make_keypair(TEST_COVENANT_KEY_SEED);
        let recovery_keypair = make_keypair(TEST_RECOVERY_KEY_SEED);
        assert_eq!(
            params.covenant_key(),
            covenant_keypair.x_only_public_key().0,
            "deposit parameters must match the fixture's covenant signing key"
        );
        let mut aux = recovery_keypair.x_only_public_key().0.serialize().to_vec();
        aux.extend_from_slice(TEST_DEPOSIT_DESTINATION);
        let header = DrtHeaderAux::from_aux_data(&aux).expect("request auxiliary data");
        let mut request_tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([3; 32]), 0),
                ..Default::default()
            }],
            output: vec![
                TxOut {
                    value: Amount::ZERO,
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: DepositTx::drt_required(params.deposit_amount()),
                    script_pubkey: create_deposit_request_locking_script(
                        header.recovery_pk(),
                        params.covenant_key(),
                        params.recovery_delay(),
                    ),
                },
            ],
        };
        set_header(&mut request_tx, params.magic(), header.build_tag_data());
        Self {
            params,
            request_tx,
            covenant_keypair,
            recovery_keypair,
        }
    }

    pub(crate) fn with_request_seed(seed: u8) -> Self {
        let mut fixture = Self::new();
        // Distinct funding outpoints give requests distinct identities under one covenant.
        fixture.request_tx.input[0].previous_output.txid = Txid::from_byte_array([seed; 32]);
        fixture
    }

    pub(crate) fn params(&self) -> &DepositParams {
        &self.params
    }

    pub(crate) fn request_tx(&self) -> &Transaction {
        &self.request_tx
    }

    pub(crate) fn request_tx_mut(&mut self) -> &mut Transaction {
        &mut self.request_tx
    }

    pub(crate) fn parser(&self) -> DepositParser {
        DepositParser::new(self.params.clone())
    }

    pub(crate) fn request(&self) -> DepositRequest {
        self.parser()
            .parse_deposit_request(&self.request_tx)
            .expect("validate request")
            .expect("bridge request")
    }

    pub(crate) fn deposit_tx(&self) -> Transaction {
        let mut tx = self.spend_tx();
        tx.output = vec![
            TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: self.params.deposit_amount(),
                script_pubkey: ScriptBuf::new_p2tr(SECP256K1, self.params.covenant_key(), None),
            },
        ];
        set_header(
            &mut tx,
            self.params.magic(),
            DepositTxHeaderAux::new(TEST_DEPOSIT_INDEX).build_tag_data(),
        );
        self.sign_key_spend(&mut tx, None);
        tx
    }

    pub(crate) fn spend_tx(&self) -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: self.request().outpoint(),
                sequence: Sequence::MAX,
                ..Default::default()
            }],
            output: vec![TxOut {
                value: self.params.deposit_amount(),
                script_pubkey: ScriptBuf::new_p2tr(
                    SECP256K1,
                    self.recovery_keypair.x_only_public_key().0,
                    None,
                ),
            }],
        }
    }

    pub(crate) fn sign_key_spend(&self, tx: &mut Transaction, annex: Option<&[u8]>) {
        tx.input[0].sequence = Sequence::MAX;
        let prevouts = [self.request_tx.output[DRT_OUTPUT_INDEX].clone()];
        let sighash = SighashCache::new(&*tx)
            .taproot_signature_hash(
                0,
                &Prevouts::All(&prevouts),
                annex.map(|bytes| Annex::new(bytes).expect("test annex")),
                None,
                TapSighashType::Default,
            )
            .expect("key-spend signature hash");
        let spend_info = build_deposit_request_spend_info(
            &self.recovery_keypair.x_only_public_key().0.serialize(),
            self.params.covenant_key(),
            self.params.recovery_delay(),
        );
        let keypair = self
            .covenant_keypair
            .tap_tweak(SECP256K1, spend_info.merkle_root());
        let signature = SECP256K1.sign_schnorr_no_aux_rand(
            &Message::from_digest(sighash.to_byte_array()),
            keypair.as_keypair(),
        );
        tx.input[0].witness = Witness::from_slice(&[signature.as_ref()]);
        if let Some(annex) = annex {
            tx.input[0].witness.push(annex);
        }
        self.assert_valid_spend_signature(tx);
    }

    pub(crate) fn sign_reclaim(&self, tx: &mut Transaction, annex: Option<&[u8]>) {
        tx.input[0].sequence = Sequence::from_height(self.params.recovery_delay());
        let spend_info = build_deposit_request_spend_info(
            &self.recovery_keypair.x_only_public_key().0.serialize(),
            self.params.covenant_key(),
            self.params.recovery_delay(),
        );
        let script_ver = spend_info
            .script_map()
            .keys()
            .next()
            .expect("recovery leaf");
        let prevouts = [self.request_tx.output[DRT_OUTPUT_INDEX].clone()];
        let sighash = SighashCache::new(&*tx)
            .taproot_signature_hash(
                0,
                &Prevouts::All(&prevouts),
                annex.map(|bytes| Annex::new(bytes).expect("test annex")),
                Some((
                    TapLeafHash::from_script(&script_ver.0, script_ver.1),
                    u32::MAX,
                )),
                TapSighashType::Default,
            )
            .expect("recovery signature hash");
        let signature = SECP256K1.sign_schnorr_no_aux_rand(
            &Message::from_digest(sighash.to_byte_array()),
            &self.recovery_keypair,
        );
        let control = spend_info
            .control_block(script_ver)
            .expect("recovery control block");
        tx.input[0].witness = Witness::from_slice(&[
            signature.as_ref(),
            script_ver.0.as_bytes(),
            &control.serialize(),
        ]);
        if let Some(annex) = annex {
            tx.input[0].witness.push(annex);
        }
        self.assert_valid_spend_signature(tx);
    }

    fn assert_valid_spend_signature(&self, tx: &Transaction) {
        // Derive the hash from the final witness so an annex cannot be added after signing.
        let witness = &tx.input[0].witness;
        let signature = Signature::from_slice(witness.nth(0).expect("spend signature"))
            .expect("Taproot signature");
        let leaf = witness.taproot_leaf_script();
        let prevouts = [self.request_tx.output[DRT_OUTPUT_INDEX].clone()];
        let sighash = SighashCache::new(tx)
            .taproot_signature_hash(
                0,
                &Prevouts::All(&prevouts),
                witness
                    .taproot_annex()
                    .map(|bytes| Annex::new(bytes).expect("test annex")),
                leaf.map(|leaf| {
                    (
                        TapLeafHash::from_script(leaf.script, leaf.version),
                        u32::MAX,
                    )
                }),
                signature.sighash_type,
            )
            .expect("final witness signature hash");
        let key = if leaf.is_some() {
            self.recovery_keypair.x_only_public_key().0
        } else {
            XOnlyPublicKey::from_slice(&prevouts[0].script_pubkey.as_bytes()[2..])
                .expect("request Taproot output key")
        };
        SECP256K1
            .verify_schnorr(
                &signature.signature,
                &Message::from_digest(sighash.to_byte_array()),
                &key,
            )
            .expect("spend signature verifies against the final witness");
    }
}

pub(crate) fn make_keypair(seed: u8) -> Keypair {
    let secret = SecretKey::from_slice(&[seed; 32]).expect("test secret key");
    Keypair::from_secret_key(SECP256K1, &secret)
}

pub(crate) fn set_header(tx: &mut Transaction, magic: MagicBytes, tag: TagData) {
    tx.output[0].script_pubkey = ParseConfig::new(magic)
        .encode_script_buf(&tag.as_ref())
        .expect("SPS-50 header");
}
