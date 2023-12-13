#![allow(dead_code)]
use ethers::types::H160;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::extractor::evm::Transaction;
use strum_macros::{Display, EnumString};

use crate::{hex_bytes::Bytes};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumString, Display, Default,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Chain {
    #[default]
    Ethereum,
    Starknet,
    ZkSync,
}

#[derive(PartialEq, Debug, Clone)]
pub enum ProtocolSystem {
    Ambient,
}

#[derive(PartialEq, Debug, Clone)]
pub enum ImplementationType {
    Vm,
    Custom,
}

#[derive(PartialEq, Debug, Clone)]
pub enum FinancialType {
    Swap,
    Lend,
    Leverage,
    Psm,
}

#[derive(PartialEq, Debug, Clone)]
pub struct ProtocolType {
    pub name: String,
    pub attribute_schema: serde_json::Value,
    pub financial_type: FinancialType,
    pub implementation_type: ImplementationType,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ExtractorIdentity {
    pub chain: Chain,
    pub name: String,
}

impl ExtractorIdentity {
    pub fn new(chain: Chain, name: &str) -> Self {
        Self { chain, name: name.to_owned() }
    }
}

impl std::fmt::Display for ExtractorIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.chain, self.name)
    }
}

#[derive(Debug, PartialEq)]
pub struct ExtractionState {
    pub name: String,
    pub chain: Chain,
    pub attributes: serde_json::Value,
    pub cursor: Vec<u8>,
}

impl ExtractionState {
    pub fn new(
        name: String,
        chain: Chain,
        attributes: Option<serde_json::Value>,
        cursor: &[u8],
    ) -> Self {
        ExtractionState {
            name,
            chain,
            attributes: attributes.unwrap_or_default(),
            cursor: cursor.to_vec(),
        }
    }
}

#[typetag::serde(tag = "type")]
pub trait NormalisedMessage: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static {
    fn source(&self) -> ExtractorIdentity;
}
/// A type representing the unique identifier for a contract. It can represent an on-chain address
/// or in the case of a one-to-many relationship it could be something like 'USDC-ETH'. This is for
/// example the case with ambient, where one component is responsible for multiple contracts.
///
/// `ContractId` is a simple wrapper around a `String` to ensure type safety
/// and clarity when working with contract identifiers.
#[derive(PartialEq, Debug)]
pub struct ContractId(pub String);

pub struct ProtocolComponent<T> {
    // an id for this component, could be hex repr of contract address
    pub id: ContractId,
    // what system this component belongs to
    pub protocol_system: ProtocolSystem,
    // more metadata information about the components general type (swap, lend, bridge, etc.)
    pub protocol_type: ProtocolType,
    // Blockchain the component belongs to
    pub chain: Chain,
    // holds the tokens tradable
    pub tokens: Vec<T>,
    // ID's referring to related contracts
    pub contract_ids: Vec<ContractId>,
    // allows to express some validation over the static attributes if necessary
    pub static_attributes: HashMap<String, Bytes>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TvlChange {
    pub token: H160,
    pub new_balance: f64,
    // tx where the this balance was observed
    pub modify_tx: String,
    pub component_id: String,
}

#[allow(dead_code)]
pub struct ProtocolState {
    // associates back to a component, which has metadata like type, tokens , etc.
    pub component_id: String,
    // holds all the protocol specific attributes, validates by the components schema
    pub attributes: HashMap<String, Bytes>,
    // via transaction, we can trace back when this state became valid
    pub modify_tx: Transaction,
}
