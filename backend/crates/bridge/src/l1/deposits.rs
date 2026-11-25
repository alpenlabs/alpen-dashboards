//! Recognition of deposit request transactions (DRTs) and deposit transactions (DTs).

use bitcoin::{
    secp256k1::{Error as Secp256k1Error, SECP256K1},
    taproot::Signature,
    Amount, OutPoint, ScriptBuf, Transaction, Txid, Witness, XOnlyPublicKey,
};
use serde::{Deserialize, Serialize};
use strata_asm_common::TxInputRef;
use strata_asm_proto_bridge_v1_txs::{
    constants::BridgeTxType,
    deposit::{parse_deposit_tx, DepositInfo},
    deposit_request::{
        build_deposit_request_spend_info, create_deposit_request_locking_script, parse_drt,
        DepositRequestInfo, DRT_OUTPUT_INDEX,
    },
    errors::TxStructureError,
    BRIDGE_V1_SUBPROTOCOL_ID,
};
use strata_bridge_tx_graph::transactions::deposit::DepositTx;
use strata_l1_txfmt::{MagicBytes, ParseConfig, TagDataRef, TxFmtError};
use thiserror::Error;

/// Protocol and covenant parameters used to validate deposit requests and spends.
#[derive(Debug, Clone)]
pub(crate) struct DepositParams {
    /// Network's SPS-50 magic bytes.
    magic: MagicBytes,

    /// Aggregate covenant key expected in the request and deposit outputs.
    covenant_key: XOnlyPublicKey,

    /// Bridge deposit denomination, excluding the deposit transaction's mining fee.
    deposit_amount: Amount,

    /// Number of blocks required by the request's recovery script.
    recovery_delay: u16,

    /// Minimum request funding, including the deposit transaction's mining fee.
    required_request_amount: Amount,
}

impl DepositParams {
    /// Creates deposit validation parameters with the protocol's deposit transaction fee.
    ///
    /// Returns an error if the denomination and fee exceed Bitcoin's maximum amount.
    pub(crate) fn new(
        magic: MagicBytes,
        covenant_key: XOnlyPublicKey,
        deposit_amount: Amount,
        recovery_delay: u16,
    ) -> Result<Self, DepositError> {
        // The public funding calculation supplies the protocol fee without duplicating it.
        let required_request_amount = deposit_amount
            .to_sat()
            .checked_add(DepositTx::drt_required(Amount::ZERO).to_sat())
            .filter(|amount| *amount <= Amount::MAX_MONEY.to_sat())
            .map(Amount::from_sat)
            .ok_or(DepositError::InvalidDepositAmount)?;
        Ok(Self {
            magic,
            covenant_key,
            deposit_amount,
            recovery_delay,
            required_request_amount,
        })
    }

    #[cfg(test)]
    pub(crate) fn magic(&self) -> MagicBytes {
        self.magic
    }

    #[cfg(test)]
    pub(crate) fn covenant_key(&self) -> XOnlyPublicKey {
        self.covenant_key
    }

    #[cfg(test)]
    pub(crate) fn deposit_amount(&self) -> Amount {
        self.deposit_amount
    }

    #[cfg(test)]
    pub(crate) fn recovery_delay(&self) -> u16 {
        self.recovery_delay
    }
}

/// Parses bridge deposit transactions and validates them against the supplied parameters.
///
/// Inputs must come from blocks validated by a Bitcoin full node, which checks
/// consensus rules, signatures and relative timelocks. This parser checks bridge
/// structure and scripts using caller-supplied parameters. Historical requests
/// require the covenant applicable at their inclusion height.
///
/// Discovery, inclusion tracking and persistence belong to the caller.
#[derive(Debug)]
pub(crate) struct DepositParser {
    params: DepositParams,
    parse_config: ParseConfig,
}

impl DepositParser {
    /// Creates a parser for the supplied protocol and covenant parameters.
    pub(crate) fn new(params: DepositParams) -> Self {
        let parse_config = ParseConfig::new(params.magic);
        Self {
            params,
            parse_config,
        }
    }

    /// Recognizes and validates a deposit request for this network and covenant.
    ///
    /// Returns `Ok(None)` if the transaction has no protocol header or its tag belongs
    /// to another network, subprotocol, or transaction type.
    ///
    /// Malformed OP_RETURN headers return an error even when they cannot be identified
    /// as bridge candidates. Identified requests that fail structural or protocol
    /// validation also return an error.
    pub(crate) fn parse_deposit_request(
        &self,
        tx: &Transaction,
    ) -> Result<Option<DepositRequest>, DepositError> {
        if self.bridge_tag(tx, BridgeTxType::DepositRequest)?.is_none() {
            return Ok(None);
        }
        let info = parse_drt(tx)?;
        validate_request(&info, &self.params)?;
        Ok(Some(DepositRequest {
            outpoint: OutPoint::new(tx.compute_txid(), DRT_OUTPUT_INDEX as u32),
            magic: self.params.magic,
            info,
        }))
    }

    /// Returns the request outpoint from a structurally valid DT for this network.
    ///
    /// Unrelated transactions and script-path spends return `Ok(None)`; malformed
    /// headers or DT structures return an error. The recovered request and its
    /// covenant must still be validated with [`Self::parse_deposit_request`] and
    /// [`Self::parse_deposit_request_spend`].
    pub(crate) fn parse_deposit_spent_request(
        &self,
        tx: &Transaction,
    ) -> Result<Option<OutPoint>, DepositError> {
        // A recovery spend may carry a DT tag, but does not require request recovery.
        if tx
            .input
            .first()
            .is_some_and(|input| input.witness.taproot_leaf_script().is_some())
        {
            return Ok(None);
        }
        Ok(self.parse_deposit(tx)?.map(|info| *info.drt_inpoint()))
    }

    /// Classifies a spend of a previously validated request.
    ///
    /// Returns `Ok(None)` if no input spends the request. Otherwise returns its
    /// classification, including `Other` when the spend establishes neither a deposit
    /// transaction nor the committed recovery path.
    ///
    /// A matching recovery script takes precedence over a deposit transaction tag:
    /// the depositor can also construct an OP_RETURN.
    ///
    /// Incompatible request parameters, malformed OP_RETURN headers, and deposit
    /// candidates that fail validation return an error. A malformed header need not
    /// belong to this network or bridge.
    pub(crate) fn parse_deposit_request_spend(
        &self,
        request: &DepositRequest,
        tx: &Transaction,
    ) -> Result<Option<DepositSpend>, DepositError> {
        let Some(input) = tx
            .input
            .iter()
            .find(|input| input.previous_output == request.outpoint)
        else {
            return Ok(None);
        };
        if request.magic != self.params.magic {
            return Err(DepositError::RequestNetworkMismatch);
        }
        validate_request(&request.info, &self.params)?;
        let txid = tx.compute_txid();
        let spend_info = build_deposit_request_spend_info(
            request.info.header_aux().recovery_pk(),
            self.params.covenant_key,
            self.params.recovery_delay,
        );
        // Compare both the leaf and its commitment; arbitrary script-path spends are not reclaims.
        if let Some(leaf) = input.witness.taproot_leaf_script() {
            let script_ver = (leaf.script.to_owned(), leaf.version);
            if spend_info
                .control_block(&script_ver)
                .is_some_and(|control| {
                    input.witness.taproot_control_block() == Some(control.serialize().as_slice())
                })
            {
                return Ok(Some(DepositSpend::Reclaim { txid }));
            }
            return Ok(Some(DepositSpend::Other { txid }));
        }
        let Some(info) = self.parse_deposit(tx)? else {
            return Ok(Some(DepositSpend::Other { txid }));
        };
        validate_deposit(&info, request, &input.witness, &self.params)?;
        Ok(Some(DepositSpend::Deposit {
            txid,
            deposit_idx: info.header_aux().deposit_idx(),
        }))
    }

    fn parse_deposit(&self, tx: &Transaction) -> Result<Option<DepositInfo>, DepositError> {
        let Some(tag) = self.bridge_tag(tx, BridgeTxType::Deposit)? else {
            return Ok(None);
        };
        Ok(Some(parse_deposit_tx(&TxInputRef::new(tx, tag))?))
    }

    fn bridge_tag<'tx>(
        &self,
        tx: &'tx Transaction,
        tx_type: BridgeTxType,
    ) -> Result<Option<TagDataRef<'tx>>, DepositError> {
        let tag = match self.parse_config.try_parse_tx(tx) {
            Ok(tag) => tag,
            Err(
                TxFmtError::MissingOutput0 | TxFmtError::NotOpret | TxFmtError::MismatchMagic(_),
            ) => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        Ok(
            (tag.subproto_id() == BRIDGE_V1_SUBPROTOCOL_ID && tag.tx_type() == tx_type as u8)
                .then_some(tag),
        )
    }
}

/// A validated request identified before a bridge deposit index is assigned.
#[derive(Debug, Clone)]
pub(crate) struct DepositRequest {
    outpoint: OutPoint,
    magic: MagicBytes,
    info: DepositRequestInfo,
}

impl DepositRequest {
    /// Stable deposit identity, independent of bridge indexing.
    pub(crate) fn outpoint(&self) -> OutPoint {
        self.outpoint
    }
}

/// Classification of a transaction that spends a validated request output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DepositSpend {
    /// Bridge deposit transaction using the covenant's key-spend path.
    Deposit { txid: Txid, deposit_idx: u32 },

    /// Spend through the request's committed recovery script.
    Reclaim { txid: Txid },

    /// Spend that does not establish a deposit transaction or the expected recovery path.
    Other { txid: Txid },
}

/// Errors constructing deposit parameters or parsing and validating transactions.
#[derive(Debug, Error)]
pub enum DepositError {
    #[error("invalid SPS-50 envelope: {0}")]
    Envelope(#[from] TxFmtError),

    #[error("invalid bridge transaction structure: {0}")]
    Structure(#[from] TxStructureError),

    #[error("invalid request recovery public key: {0}")]
    RecoveryKey(#[from] Secp256k1Error),

    #[error("deposit denomination and required fee exceed Bitcoin's maximum amount")]
    InvalidDepositAmount,

    #[error("request funding is below the required {required}")]
    InsufficientRequestFunding { required: Amount },

    #[error("request locking script does not match the covenant and recovery parameters")]
    RequestScriptMismatch,

    #[error("request belongs to a different network")]
    RequestNetworkMismatch,

    #[error("deposit transaction's first input does not spend the validated request outpoint")]
    RequestOutpointMismatch,

    #[error("deposit transaction does not use the covenant key-spend path")]
    DepositSpendPathMismatch,

    #[error("deposit transaction output does not match the deposit denomination")]
    DepositAmountMismatch,

    #[error("deposit transaction output is not locked to the expected covenant")]
    DepositScriptMismatch,
}

fn validate_request(info: &DepositRequestInfo, params: &DepositParams) -> Result<(), DepositError> {
    XOnlyPublicKey::from_slice(info.header_aux().recovery_pk())?;
    if info.deposit_request_output().inner().value < params.required_request_amount {
        return Err(DepositError::InsufficientRequestFunding {
            required: params.required_request_amount,
        });
    }
    let expected = create_deposit_request_locking_script(
        info.header_aux().recovery_pk(),
        params.covenant_key,
        params.recovery_delay,
    );
    if info.deposit_request_output().inner().script_pubkey != expected {
        return Err(DepositError::RequestScriptMismatch);
    }
    Ok(())
}

fn validate_deposit(
    info: &DepositInfo,
    request: &DepositRequest,
    witness: &Witness,
    params: &DepositParams,
) -> Result<(), DepositError> {
    if *info.drt_inpoint() != request.outpoint {
        return Err(DepositError::RequestOutpointMismatch);
    }
    let signature_elements = witness.len() - usize::from(witness.taproot_annex().is_some());
    if signature_elements != 1
        || !witness
            .nth(0)
            .is_some_and(|sig| Signature::from_slice(sig).is_ok())
    {
        return Err(DepositError::DepositSpendPathMismatch);
    }
    if info.amt().to_sat() != params.deposit_amount.to_sat() {
        return Err(DepositError::DepositAmountMismatch);
    }
    if info.locked_script() != &ScriptBuf::new_p2tr(SECP256K1, params.covenant_key, None) {
        return Err(DepositError::DepositScriptMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bitcoin::{Amount, OutPoint, ScriptBuf, TxIn, Witness};
    use strata_asm_proto_bridge_v1_txs::{
        constants::BridgeTxType, deposit::DepositTxHeaderAux, deposit_request::DRT_OUTPUT_INDEX,
        BRIDGE_V1_SUBPROTOCOL_ID,
    };
    use strata_bridge_tx_graph::transactions::deposit::DepositTx;
    use strata_l1_txfmt::{extract_tx_magic_and_tag, MagicBytes, TagData};

    use super::{DepositError, DepositParams, DepositParser, DepositSpend};
    use crate::l1::test_utils::{
        make_keypair, set_header, DepositFixture, TEST_ANNEX, TEST_DEPOSIT_DESTINATION,
        TEST_DEPOSIT_INDEX,
    };

    #[test]
    fn deposit_request_preserves_outpoint_funding_and_destination() {
        let fixture = DepositFixture::new();
        let request = fixture.request();

        // Identity exists before the protocol assigns a deposit index.
        assert_eq!(
            request.outpoint(),
            OutPoint::new(fixture.request_tx().compute_txid(), DRT_OUTPUT_INDEX as u32)
        );
        assert_eq!(
            request.info.deposit_request_output().inner().value,
            DepositTx::drt_required(fixture.params().deposit_amount())
        );
        assert_eq!(
            request.info.header_aux().destination().as_slice(),
            TEST_DEPOSIT_DESTINATION
        );
    }

    #[test]
    fn deposit_request_ignores_other_networks_subprotocols_and_transaction_types() {
        let fixture = DepositFixture::new();
        let (_, tag) = extract_tx_magic_and_tag(fixture.request_tx()).expect("request header");
        let aux = tag.aux_data().to_vec();
        for (magic, subprotocol, tx_type) in [
            (
                MagicBytes::new(*b"OTHR"),
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::DepositRequest as u8,
            ),
            (
                fixture.params().magic(),
                BRIDGE_V1_SUBPROTOCOL_ID + 1,
                BridgeTxType::DepositRequest as u8,
            ),
            (
                fixture.params().magic(),
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::Deposit as u8,
            ),
        ] {
            let mut tx = fixture.request_tx().clone();
            set_header(
                &mut tx,
                magic,
                TagData::new(subprotocol, tx_type, aux.clone()).expect("tag"),
            );
            assert!(fixture
                .parser()
                .parse_deposit_request(&tx)
                .expect("unrelated tag")
                .is_none());
        }
    }

    #[test]
    fn deposit_request_ignores_transactions_without_a_protocol_header() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.request_tx().clone();
        tx.output[0].script_pubkey = ScriptBuf::new();
        assert!(fixture
            .parser()
            .parse_deposit_request(&tx)
            .expect("ordinary transaction")
            .is_none());
        tx.output.clear();
        assert!(fixture
            .parser()
            .parse_deposit_request(&tx)
            .expect("no header output")
            .is_none());
    }

    #[test]
    fn deposit_request_rejects_malformed_envelopes_and_auxiliary_data() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.request_tx().clone();
        tx.output[0].script_pubkey = ScriptBuf::from_bytes(vec![0x6a]);
        assert!(matches!(
            fixture.parser().parse_deposit_request(&tx),
            Err(DepositError::Envelope(_))
        ));

        // A matching transaction type still needs a complete recovery key.
        set_header(
            &mut tx,
            fixture.params().magic(),
            TagData::new(
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::DepositRequest as u8,
                vec![0; 31],
            )
            .expect("short request tag"),
        );
        assert!(matches!(
            fixture.parser().parse_deposit_request(&tx),
            Err(DepositError::Structure(_))
        ));
    }

    #[test]
    fn deposit_request_rejects_invalid_recovery_keys() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.request_tx().clone();
        set_header(
            &mut tx,
            fixture.params().magic(),
            TagData::new(
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::DepositRequest as u8,
                vec![0xff; 32],
            )
            .expect("invalid recovery key tag"),
        );
        assert!(matches!(
            fixture.parser().parse_deposit_request(&tx),
            Err(DepositError::RecoveryKey(_))
        ));
    }

    #[test]
    fn deposit_request_rejects_missing_outputs_and_insufficient_fee_funding() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.request_tx().clone();
        tx.output.pop();
        assert!(matches!(
            fixture.parser().parse_deposit_request(&tx),
            Err(DepositError::Structure(_))
        ));

        // The denomination alone is insufficient; the request must fund the DT fee too.
        let mut tx = fixture.request_tx().clone();
        tx.output[DRT_OUTPUT_INDEX].value = fixture.params().deposit_amount();
        assert!(matches!(fixture.parser().parse_deposit_request(&tx),
            Err(DepositError::InsufficientRequestFunding { required })
                if required == DepositTx::drt_required(fixture.params().deposit_amount())));
    }

    #[test]
    fn deposit_request_accepts_funding_above_the_required_amount() {
        let mut fixture = DepositFixture::new();
        fixture.request_tx_mut().output[DRT_OUTPUT_INDEX].value += Amount::from_sat(1_000);
        assert_eq!(
            fixture
                .request()
                .info
                .deposit_request_output()
                .inner()
                .value,
            fixture.request_tx().output[DRT_OUTPUT_INDEX].value
        );
    }

    #[test]
    fn deposit_request_rejects_wrong_covenants_recovery_delays_and_scripts() {
        let fixture = DepositFixture::new();
        let params = DepositParams::new(
            fixture.params().magic(),
            make_keypair(4).x_only_public_key().0,
            fixture.params().deposit_amount(),
            fixture.params().recovery_delay(),
        )
        .expect("alternate covenant parameters");
        assert!(matches!(
            DepositParser::new(params).parse_deposit_request(fixture.request_tx()),
            Err(DepositError::RequestScriptMismatch)
        ));
        let params = DepositParams::new(
            fixture.params().magic(),
            fixture.params().covenant_key(),
            fixture.params().deposit_amount(),
            fixture.params().recovery_delay() + 1,
        )
        .expect("alternate recovery parameters");
        assert!(matches!(
            DepositParser::new(params).parse_deposit_request(fixture.request_tx()),
            Err(DepositError::RequestScriptMismatch)
        ));
        let mut tx = fixture.request_tx().clone();
        tx.output[DRT_OUTPUT_INDEX].script_pubkey = ScriptBuf::new();
        assert!(matches!(
            fixture.parser().parse_deposit_request(&tx),
            Err(DepositError::RequestScriptMismatch)
        ));
    }

    #[test]
    fn deposit_params_reject_unrepresentable_funding() {
        let fixture = DepositFixture::new();
        for amount in [Amount::MAX_MONEY, Amount::from_sat(u64::MAX)] {
            assert!(matches!(
                DepositParams::new(
                    fixture.params().magic(),
                    fixture.params().covenant_key(),
                    amount,
                    fixture.params().recovery_delay(),
                ),
                Err(DepositError::InvalidDepositAmount)
            ));
        }
    }

    #[test]
    fn deposit_request_outpoint_can_be_parsed_before_resolving_the_covenant() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        fixture.sign_key_spend(&mut tx, Some(TEST_ANNEX));
        let params = DepositParams::new(
            fixture.params().magic(),
            make_keypair(4).x_only_public_key().0,
            fixture.params().deposit_amount(),
            fixture.params().recovery_delay(),
        )
        .expect("alternate covenant parameters");
        let parser = DepositParser::new(params);

        // Discovery needs only the network and DT structure. The request's
        // inclusion height determines the covenant used for subsequent validation.
        assert_eq!(
            parser
                .parse_deposit_spent_request(&tx)
                .expect("DT request outpoint"),
            Some(OutPoint::new(
                fixture.request_tx().compute_txid(),
                DRT_OUTPUT_INDEX as u32
            ))
        );
        assert!(matches!(
            parser.parse_deposit_request(fixture.request_tx()),
            Err(DepositError::RequestScriptMismatch)
        ));
    }

    #[test]
    fn deposit_request_outpoint_ignores_unrelated_transactions() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        for (magic, subprotocol, tx_type) in [
            (
                MagicBytes::new(*b"OTHR"),
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::Deposit as u8,
            ),
            (
                fixture.params().magic(),
                BRIDGE_V1_SUBPROTOCOL_ID + 1,
                BridgeTxType::Deposit as u8,
            ),
            (
                fixture.params().magic(),
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::DepositRequest as u8,
            ),
        ] {
            set_header(
                &mut tx,
                magic,
                TagData::new(
                    subprotocol,
                    tx_type,
                    TEST_DEPOSIT_INDEX.to_le_bytes().to_vec(),
                )
                .expect("tag"),
            );
            fixture.sign_key_spend(&mut tx, None);
            assert_eq!(
                fixture
                    .parser()
                    .parse_deposit_spent_request(&tx)
                    .expect("unrelated tag"),
                None
            );
        }
        tx.output[0].script_pubkey = ScriptBuf::new();
        fixture.sign_key_spend(&mut tx, None);
        assert_eq!(
            fixture
                .parser()
                .parse_deposit_spent_request(&tx)
                .expect("no protocol header"),
            None
        );
    }

    #[test]
    fn deposit_request_outpoint_rejects_malformed_transactions() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        tx.input.clear();
        assert!(matches!(
            fixture.parser().parse_deposit_spent_request(&tx),
            Err(DepositError::Structure(_))
        ));

        let mut tx = fixture.deposit_tx();
        tx.output.pop();
        assert!(matches!(
            fixture.parser().parse_deposit_spent_request(&tx),
            Err(DepositError::Structure(_))
        ));

        let mut tx = fixture.deposit_tx();
        set_header(
            &mut tx,
            fixture.params().magic(),
            TagData::new(
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::Deposit as u8,
                vec![0; 5],
            )
            .expect("malformed DT tag"),
        );
        assert!(matches!(
            fixture.parser().parse_deposit_spent_request(&tx),
            Err(DepositError::Structure(_))
        ));

        tx.output[0].script_pubkey = ScriptBuf::from_bytes(vec![0x6a]);
        assert!(matches!(
            fixture.parser().parse_deposit_spent_request(&tx),
            Err(DepositError::Envelope(_))
        ));
    }

    #[test]
    fn deposit_request_outpoint_ignores_recovery_spends_with_a_dt_tag() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        for annex in [None, Some(TEST_ANNEX)] {
            fixture.sign_reclaim(&mut tx, annex);
            assert_eq!(
                fixture
                    .parser()
                    .parse_deposit_spent_request(&tx)
                    .expect("recovery spend"),
                None
            );
        }
    }

    #[test]
    fn deposit_transaction_links_its_index_and_txid_to_the_request_outpoint() {
        let fixture = DepositFixture::new();
        let params = DepositParams::new(
            MagicBytes::new(*b"OTHR"),
            fixture.params().covenant_key(),
            Amount::from_sat(200_000_000),
            12,
        )
        .expect("alternate deposit parameters");

        // Request construction, signing and parsing must agree for either parameter set.
        for fixture in [fixture, DepositFixture::with_params(params)] {
            let tx = fixture.deposit_tx();
            let result = fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx)
                .expect("valid DT")
                .expect("spends request");
            assert!(matches!(result, DepositSpend::Deposit { txid, deposit_idx }
                if txid == tx.compute_txid() && deposit_idx == TEST_DEPOSIT_INDEX));
        }
    }

    #[test]
    fn deposit_spend_does_not_mix_requests_with_the_same_output_index() {
        let first = DepositFixture::new();
        let second = DepositFixture::with_request_seed(4);
        let first_request = first.request();
        let second_request = second.request();

        // Both requests use the same covenant and output index, but distinct transaction IDs.
        assert_eq!(
            first_request.outpoint().vout,
            second_request.outpoint().vout
        );
        assert_ne!(
            first_request.outpoint().txid,
            second_request.outpoint().txid
        );
        let first_spend = first.deposit_tx();
        let second_spend = second.deposit_tx();

        // Each spend is accepted for its own request and ignored for the other request.
        assert!(matches!(
            first.parser().parse_deposit_request_spend(&first_request, &first_spend),
            Ok(Some(DepositSpend::Deposit { txid, .. })) if txid == first_spend.compute_txid()
        ));
        assert!(matches!(
            second.parser().parse_deposit_request_spend(&second_request, &second_spend),
            Ok(Some(DepositSpend::Deposit { txid, .. })) if txid == second_spend.compute_txid()
        ));
        assert!(first
            .parser()
            .parse_deposit_request_spend(&first_request, &second_spend)
            .expect("other request's spend")
            .is_none());
        assert!(second
            .parser()
            .parse_deposit_request_spend(&second_request, &first_spend)
            .expect("other request's spend")
            .is_none());
    }

    #[test]
    fn deposit_transaction_ignores_other_request_outpoints() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        // Even another output of the same DRT is a different deposit identity.
        tx.input[0].previous_output.vout += 1;
        assert!(fixture
            .parser()
            .parse_deposit_request_spend(&fixture.request(), &tx)
            .expect("different outpoint")
            .is_none());
        tx.input.clear();
        assert!(fixture
            .parser()
            .parse_deposit_request_spend(&fixture.request(), &tx)
            .expect("no request input")
            .is_none());
    }

    #[test]
    fn deposit_transaction_rejects_a_request_outside_the_first_input() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        tx.input.insert(
            0,
            TxIn {
                previous_output: OutPoint::null(),
                ..Default::default()
            },
        );
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Err(DepositError::RequestOutpointMismatch)
        ));
    }

    #[test]
    fn deposit_transaction_rejects_malformed_auxiliary_data_and_missing_outputs() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        set_header(
            &mut tx,
            fixture.params().magic(),
            TagData::new(
                BRIDGE_V1_SUBPROTOCOL_ID,
                BridgeTxType::Deposit as u8,
                vec![0; 5],
            )
            .expect("DT tag with trailing data"),
        );
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Err(DepositError::Structure(_))
        ));
        let mut tx = fixture.deposit_tx();
        tx.output.pop();
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Err(DepositError::Structure(_))
        ));
    }

    #[test]
    fn deposit_transaction_rejects_wrong_amounts_and_covenant_outputs() {
        let fixture = DepositFixture::new();
        for amount in [
            Amount::from_sat(1),
            fixture.params().deposit_amount() + Amount::ONE_SAT,
        ] {
            let mut tx = fixture.deposit_tx();
            tx.output[1].value = amount;
            assert!(matches!(
                fixture
                    .parser()
                    .parse_deposit_request_spend(&fixture.request(), &tx),
                Err(DepositError::DepositAmountMismatch)
            ));
        }
        let mut tx = fixture.deposit_tx();
        tx.output[1].script_pubkey = ScriptBuf::new();
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Err(DepositError::DepositScriptMismatch)
        ));
    }

    #[test]
    fn deposit_transaction_accepts_a_key_spend_with_an_annex() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();

        // The final witness includes the annex and a signature that commits to it.
        fixture.sign_key_spend(&mut tx, Some(TEST_ANNEX));
        assert_eq!(tx.input[0].witness.taproot_annex(), Some(TEST_ANNEX));
        assert!(matches!(
            fixture.parser().parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Deposit { txid, deposit_idx }))
                if txid == tx.compute_txid() && deposit_idx == TEST_DEPOSIT_INDEX
        ));
    }

    #[test]
    fn deposit_transaction_requires_a_key_spend_witness() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        tx.input[0].witness.clear();
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Err(DepositError::DepositSpendPathMismatch)
        ));
    }

    #[test]
    fn deposit_spend_without_a_deposit_tag_is_classified_as_other() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.spend_tx();

        // Covenant authorization alone does not establish a bridge deposit transaction.
        fixture.sign_key_spend(&mut tx, None);
        assert!(matches!(
            fixture.parser().parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Other { txid })) if txid == tx.compute_txid()
        ));
    }

    #[test]
    fn deposit_spend_revalidates_the_request_against_supplied_params() {
        let fixture = DepositFixture::new();
        let request = fixture.request();
        let tx = fixture.deposit_tx();
        let params = DepositParams::new(
            MagicBytes::new(*b"OTHR"),
            fixture.params().covenant_key(),
            fixture.params().deposit_amount(),
            fixture.params().recovery_delay(),
        )
        .expect("alternate network parameters");
        assert!(matches!(
            DepositParser::new(params).parse_deposit_request_spend(&request, &tx),
            Err(DepositError::RequestNetworkMismatch)
        ));
        let params = DepositParams::new(
            fixture.params().magic(),
            make_keypair(4).x_only_public_key().0,
            fixture.params().deposit_amount(),
            fixture.params().recovery_delay(),
        )
        .expect("alternate covenant parameters");
        assert!(matches!(
            DepositParser::new(params).parse_deposit_request_spend(&request, &tx),
            Err(DepositError::RequestScriptMismatch)
        ));
    }

    #[test]
    fn deposit_spend_identifies_the_committed_recovery_path() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.spend_tx();
        fixture.sign_reclaim(&mut tx, None);
        assert!(
            matches!(fixture.parser().parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Reclaim { txid })) if txid == tx.compute_txid())
        );

        // Re-sign with the annex included in the signature hash.
        fixture.sign_reclaim(&mut tx, Some(TEST_ANNEX));
        assert_eq!(tx.input[0].witness.taproot_annex(), Some(TEST_ANNEX));
        assert!(matches!(
            fixture.parser().parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Reclaim { txid })) if txid == tx.compute_txid()
        ));
    }

    #[test]
    fn deposit_spend_treats_a_recovery_spend_with_a_dt_tag_as_reclaim() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        // The depositor can copy the DT's outputs and header without covenant approval.
        fixture.sign_reclaim(&mut tx, None);
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Reclaim { .. }))
        ));
    }

    #[test]
    fn deposit_spend_does_not_label_an_unrecognized_path_as_reclaim() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.spend_tx();
        fixture.sign_reclaim(&mut tx, None);
        let mut elements = tx.input[0].witness.to_vec();
        // A different control block does not prove the expected recovery commitment.
        elements[2][1] ^= 1;
        tx.input[0].witness = Witness::from_slice(&elements);
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Other { .. }))
        ));
        tx.input[0].witness.clear();
        assert!(matches!(
            fixture
                .parser()
                .parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Other { .. }))
        ));
    }

    #[test]
    fn deposit_spend_does_not_treat_another_networks_dt_as_a_deposit() {
        let fixture = DepositFixture::new();
        let mut tx = fixture.deposit_tx();
        set_header(
            &mut tx,
            MagicBytes::new(*b"OTHR"),
            DepositTxHeaderAux::new(TEST_DEPOSIT_INDEX).build_tag_data(),
        );
        fixture.sign_key_spend(&mut tx, None);
        assert!(matches!(
            fixture.parser().parse_deposit_request_spend(&fixture.request(), &tx),
            Ok(Some(DepositSpend::Other { txid })) if txid == tx.compute_txid()
        ));
    }
}
