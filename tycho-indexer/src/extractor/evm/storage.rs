#![allow(unused_variables)]

use crate::{
    extractor::{
        evm,
        evm::utils::{parse_u256_slot_entry, TryDecode},
    },
    models::Chain,
    storage,
    storage::{
        ContractDelta, ContractStore, StorableBlock, StorableContract, StorableTransaction,
        StorageError,
    },
};
use chrono::NaiveDateTime;
use std::collections::HashMap;

pub mod pg {
    use crate::{
        extractor::evm::{
            utils::{convert_addresses_to_h160, pad_and_parse_h160},
            TokenQuality,
        },
        models,
        models::{FinancialType, ImplementationType},
        storage::{
            postgres::{
                orm,
                orm::{NewToken, Token},
            },
            Address, Balance, BlockHash, ChangeType, Code, StorableComponentBalance,
            StorableProtocolComponent, StorableProtocolState, StorableProtocolType, StorableToken,
            TxHash,
        },
    };
    use ethers::types::{H160, H256, U256};
    use tycho_types::Bytes;

    use self::storage::{AttrStoreKey, StorableProtocolStateDelta};

    use super::*;

    impl From<evm::Account> for evm::AccountUpdate {
        fn from(value: evm::Account) -> Self {
            evm::AccountUpdate::new(
                value.address,
                value.chain,
                value.slots,
                Some(value.balance),
                Some(value.code),
                ChangeType::Creation,
            )
        }
    }

    impl StorableBlock<orm::Block, orm::NewBlock, i64> for evm::Block {
        fn from_storage(val: orm::Block, chain: Chain) -> Result<Self, StorageError> {
            Ok(evm::Block {
                number: val.number as u64,
                hash: H256::try_decode(&val.hash, "block hash")
                    .map_err(StorageError::DecodeError)?,
                parent_hash: H256::try_decode(&val.parent_hash, "parent hash")
                    .map_err(|err| StorageError::DecodeError(err.to_string()))?,
                chain,
                ts: val.ts,
            })
        }

        fn to_storage(&self, chain_id: i64) -> orm::NewBlock {
            orm::NewBlock {
                hash: self.hash.into(),
                parent_hash: self.parent_hash.into(),
                chain_id,
                main: false,
                number: self.number as i64,
                ts: self.ts,
            }
        }

        fn chain(&self) -> &Chain {
            &self.chain
        }
    }

    impl StorableTransaction<orm::Transaction, orm::NewTransaction, i64> for evm::Transaction {
        fn from_storage(
            val: orm::Transaction,
            block_hash: &BlockHash,
        ) -> Result<Self, StorageError> {
            let to = if !val.to.is_empty() {
                Some(H160::try_decode(&val.to, "tx receiver").map_err(StorageError::DecodeError)?)
            } else {
                None
            };
            Ok(Self::new(
                H256::try_decode(&val.hash, "tx hash").map_err(StorageError::DecodeError)?,
                H256::try_decode(block_hash, "tx block hash").map_err(StorageError::DecodeError)?,
                H160::try_decode(&val.from, "tx sender").map_err(StorageError::DecodeError)?,
                to,
                val.index as u64,
            ))
        }

        fn to_storage(&self, block_id: i64) -> orm::NewTransaction {
            let to: Address = self
                .to
                .map(|v| v.into())
                .unwrap_or_default();
            orm::NewTransaction {
                hash: self.hash.into(),
                block_id,
                from: self.from.into(),
                to,
                index: self.index as i64,
            }
        }

        fn block_hash(&self) -> BlockHash {
            self.block_hash.into()
        }

        fn hash(&self) -> BlockHash {
            self.hash.into()
        }
    }
    impl StorableProtocolType<orm::ProtocolType, orm::NewProtocolType, i64> for models::ProtocolType {
        fn from_storage(val: orm::ProtocolType) -> Result<Self, StorageError> {
            let financial_type: FinancialType = match val.financial_type {
                orm::FinancialType::Swap => FinancialType::Swap,
                orm::FinancialType::Psm => FinancialType::Psm,
                orm::FinancialType::Debt => FinancialType::Debt,
                orm::FinancialType::Leverage => FinancialType::Leverage,
            };
            let implementation_type: ImplementationType = match val.implementation {
                orm::ImplementationType::Custom => ImplementationType::Custom,
                orm::ImplementationType::Vm => ImplementationType::Vm,
            };

            Ok(Self::new(val.name, financial_type, val.attribute_schema, implementation_type))
        }

        fn to_storage(&self) -> orm::NewProtocolType {
            let financial_protocol_type: orm::FinancialType = match self.financial_type {
                FinancialType::Swap => orm::FinancialType::Swap,
                FinancialType::Psm => orm::FinancialType::Psm,
                FinancialType::Debt => orm::FinancialType::Debt,
                FinancialType::Leverage => orm::FinancialType::Leverage,
            };

            let protocol_implementation_type: orm::ImplementationType = match self.implementation {
                ImplementationType::Custom => orm::ImplementationType::Custom,
                ImplementationType::Vm => orm::ImplementationType::Vm,
            };

            orm::NewProtocolType {
                name: self.name.clone(),
                implementation: protocol_implementation_type,
                attribute_schema: self.attribute_schema.clone(),
                financial_type: financial_protocol_type,
            }
        }
    }
    impl StorableComponentBalance<orm::ComponentBalance, orm::NewComponentBalance, i64>
        for evm::ComponentBalance
    {
        fn to_storage(
            &self,
            token_id: i64,
            modify_tx: i64,
            protocol_component_id: i64,
            block_ts: NaiveDateTime,
        ) -> orm::NewComponentBalance {
            orm::NewComponentBalance {
                token_id,
                new_balance: self.balance.clone(),
                previous_value: Balance::from(U256::from(0)),
                balance_float: self.balance_float,
                modify_tx,
                protocol_component_id,
                valid_from: block_ts,
                valid_to: None,
            }
        }
        fn modify_tx(&self) -> TxHash {
            self.modify_tx.into()
        }

        fn token(&self) -> Address {
            self.token.into()
        }
    }

    impl StorableContract<orm::Contract, orm::NewContract, i64> for evm::Account {
        fn from_storage(
            val: orm::Contract,
            chain: Chain,
            balance_modify_tx: &TxHash,
            code_modify_tx: &TxHash,
            creation_tx: Option<&TxHash>,
        ) -> Result<Self, StorageError> {
            Ok(evm::Account::new(
                chain,
                H160::try_decode(&val.account.address, "address")
                    .map_err(StorageError::DecodeError)?,
                val.account.title.clone(),
                HashMap::new(),
                U256::try_decode(&val.balance.balance, "balance")
                    .map_err(StorageError::DecodeError)?,
                val.code.code,
                H256::try_decode(&val.code.hash, "code hash").map_err(StorageError::DecodeError)?,
                H256::try_decode(balance_modify_tx, "tx hash")
                    .map_err(StorageError::DecodeError)?,
                H256::try_decode(code_modify_tx, "tx hash").map_err(StorageError::DecodeError)?,
                match creation_tx {
                    Some(v) => H256::try_decode(v, "tx hash")
                        .map(Some)
                        .map_err(StorageError::DecodeError)?,
                    _ => None,
                },
            ))
        }

        fn to_storage(
            &self,
            chain_id: i64,
            creation_ts: NaiveDateTime,
            tx_id: Option<i64>,
        ) -> orm::NewContract {
            orm::NewContract {
                title: self.title.clone(),
                address: self.address.into(),
                chain_id,
                creation_tx: tx_id,
                created_at: Some(creation_ts),
                deleted_at: None,
                balance: self.balance.into(),
                code: self.code.clone(),
                code_hash: self.code_hash.into(),
            }
        }

        fn chain(&self) -> &Chain {
            &self.chain
        }

        fn creation_tx(&self) -> Option<TxHash> {
            self.creation_tx.map(|v| v.into())
        }

        fn address(&self) -> Address {
            self.address.into()
        }

        fn store(&self) -> ContractStore {
            self.slots
                .iter()
                .map(|(s, v)| ((*s).into(), Some((*v).into())))
                .collect()
        }

        fn set_store(&mut self, store: &ContractStore) -> Result<(), StorageError> {
            self.slots = store
                .iter()
                .map(|(rk, rv)| {
                    parse_u256_slot_entry(rk, rv.as_ref()).map_err(StorageError::DecodeError)
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            Ok(())
        }
    }

    impl ContractDelta for evm::AccountUpdate {
        fn from_storage(
            chain: &Chain,
            address: &Address,
            slots: Option<&ContractStore>,
            balance: Option<&Balance>,
            code: Option<&Code>,
            change: ChangeType,
        ) -> Result<Self, StorageError> {
            let slots = slots
                .map(|s| {
                    s.iter()
                        .map(|(s, v)| {
                            parse_u256_slot_entry(s, v.as_ref()).map_err(StorageError::DecodeError)
                        })
                        .collect::<Result<HashMap<U256, U256>, StorageError>>()
                })
                .unwrap_or_else(|| Ok(HashMap::new()))?;

            let update = evm::AccountUpdate::new(
                H160::try_decode(address, "address").map_err(StorageError::DecodeError)?,
                *chain,
                slots,
                match balance {
                    // match expr is required so the error can be raised
                    Some(v) => U256::try_decode(v, "balance")
                        .map(Some)
                        .map_err(|err| StorageError::DecodeError(err.to_string()))?,
                    _ => None,
                },
                code.cloned(),
                change,
            );
            Ok(update)
        }

        fn contract_id(&self) -> storage::ContractId {
            storage::ContractId::new(self.chain, self.address.into())
        }

        fn dirty_balance(&self) -> Option<Balance> {
            self.balance.map(|b| b.into())
        }

        fn dirty_code(&self) -> Option<&Code> {
            self.code.as_ref()
        }

        fn dirty_slots(&self) -> ContractStore {
            self.slots
                .iter()
                .map(|(s, v)| ((*s).into(), Some((*v).into())))
                .collect()
        }
    }

    impl StorableToken<orm::Token, orm::NewToken, i64> for evm::ERC20Token {
        fn from_storage(val: Token, contract: storage::ContractId) -> Result<Self, StorageError> {
            let address =
                pad_and_parse_h160(contract.address()).map_err(StorageError::DecodeError)?;
            let quality: TokenQuality = match val.quality {
                orm::TokenQuality::Normal => TokenQuality::Normal,
                orm::TokenQuality::Rebase => TokenQuality::Rebase,
                orm::TokenQuality::Tax => TokenQuality::Tax,
                orm::TokenQuality::Scam => TokenQuality::Scam,
            };
            Ok(evm::ERC20Token::new(
                address,
                val.symbol,
                val.decimals as u32,
                val.tax as u64,
                val.gas
                    .into_iter()
                    .map(|item| item.map(|i| i as u64))
                    .collect(),
                contract.chain,
                quality,
            ))
        }

        fn to_storage(&self, contract_id: i64) -> orm::NewToken {
            let orm_quality: orm::TokenQuality = match self.quality {
                TokenQuality::Normal => orm::TokenQuality::Normal,
                TokenQuality::Rebase => orm::TokenQuality::Rebase,
                TokenQuality::Tax => orm::TokenQuality::Tax,
                TokenQuality::Scam => orm::TokenQuality::Scam,
            };
            NewToken {
                account_id: contract_id,
                symbol: self.symbol.clone(),
                decimals: self.decimals as i32,
                tax: self.tax as i64,
                gas: self
                    .gas
                    .clone()
                    .into_iter()
                    .map(|item| item.map(|i| i as i64))
                    .collect(),
                quality: orm_quality,
            }
        }

        fn chain(&self) -> Chain {
            self.chain
        }

        fn address(&self) -> H160 {
            self.address
        }

        fn symbol(&self) -> String {
            self.symbol.clone()
        }
    }

    impl StorableProtocolComponent<orm::ProtocolComponent, orm::NewProtocolComponent, i64>
        for evm::ProtocolComponent
    {
        fn from_storage(
            val: orm::ProtocolComponent,
            tokens: &[Address],
            contract_ids: &[Address],
            chain: Chain,
            protocol_system: &str,
            transaction_hash: H256,
        ) -> Result<Self, StorageError> {
            let static_attributes: HashMap<String, Bytes> = if let Some(v) = val.attributes {
                serde_json::from_value(v).map_err(|_| {
                    StorageError::DecodeError("Failed to decode static attributes.".to_string())
                })?
            } else {
                Default::default()
            };

            let token_addresses: Result<Vec<H160>, StorageError> =
                convert_addresses_to_h160(tokens);
            let contract_addresses = convert_addresses_to_h160(contract_ids);
            Ok(evm::ProtocolComponent {
                id: val.external_id,
                protocol_system: protocol_system.to_owned(),
                protocol_type_name: val.protocol_type_id.to_string(),
                chain,
                tokens: token_addresses?,
                contract_ids: contract_addresses?,
                static_attributes,
                change: Default::default(),
                creation_tx: transaction_hash,
                created_at: val.created_at,
            })
        }

        fn to_storage(
            &self,
            chain_id: i64,
            protocol_system_id: i64,
            protocol_type_id: i64,
            creation_tx: i64,
            created_at: NaiveDateTime,
        ) -> Result<orm::NewProtocolComponent, StorageError> {
            Ok(orm::NewProtocolComponent {
                external_id: self.id.clone(),
                chain_id,
                protocol_type_id,
                protocol_system_id,
                creation_tx,
                created_at,
                attributes: Some(serde_json::to_value(&self.static_attributes).map_err(|err| {
                    StorageError::DecodeError(
                        "Could not convert attributes in StorableComponent".to_string(),
                    )
                })?),
            })
        }
    }

    impl StorableProtocolState<orm::ProtocolState, orm::NewProtocolState, i64> for evm::ProtocolState {
        fn from_storage(
            vals: Vec<orm::ProtocolState>,
            component_id: String,
            tx_hash: &TxHash,
        ) -> Result<Self, StorageError> {
            let mut attr = HashMap::new();
            for val in vals {
                attr.insert(val.attribute_name, val.attribute_value);
            }
            Ok(evm::ProtocolState::new(
                component_id,
                attr,
                H256::try_decode(tx_hash, "tx hash").map_err(StorageError::DecodeError)?,
            ))
        }

        // When stored, protocol states are divided into multiple protocol state db entities - one
        // per attribute This is to allow individualised control over attribute versioning
        // and more efficient querying
        fn to_storage(
            &self,
            protocol_component_id: i64,
            tx_id: i64,
            block_ts: NaiveDateTime,
        ) -> Vec<orm::NewProtocolState> {
            let mut protocol_states = Vec::new();
            for (name, value) in &self.attributes {
                protocol_states.push(orm::NewProtocolState {
                    protocol_component_id,
                    attribute_name: Some(name.clone()),
                    attribute_value: Some(value.clone()),
                    previous_value: None,
                    modify_tx: tx_id,
                    valid_from: block_ts,
                    valid_to: None,
                });
            }
            protocol_states
        }
    }

    impl StorableProtocolStateDelta<orm::ProtocolState, orm::NewProtocolState, i64>
        for evm::ProtocolStateDelta
    {
        fn from_storage(
            vals: Vec<orm::ProtocolState>,
            component_id: String,
            deleted_attributes: Vec<AttrStoreKey>,
        ) -> Result<Self, StorageError> {
            let mut attr = HashMap::new();
            for val in vals {
                attr.insert(val.attribute_name, val.attribute_value);
            }

            Ok(evm::ProtocolStateDelta {
                component_id,
                updated_attributes: attr,
                deleted_attributes: deleted_attributes.into_iter().collect(),
            })
        }

        // When stored, protocol states are divided into multiple protocol state db entities - one
        // per attribute. This is to allow individualised control over attribute versioning
        // and more efficient querying. This function ignores deleted_attributes.
        fn to_storage(
            &self,
            protocol_component_id: i64,
            tx_id: i64,
            block_ts: NaiveDateTime,
        ) -> Vec<orm::NewProtocolState> {
            let mut protocol_states = Vec::new();
            for (name, value) in &self.updated_attributes {
                protocol_states.push(orm::NewProtocolState {
                    protocol_component_id,
                    attribute_name: Some(name.clone()),
                    attribute_value: Some(value.clone()),
                    previous_value: None,
                    modify_tx: tx_id,
                    valid_from: block_ts,
                    valid_to: None,
                });
            }
            protocol_states
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        extractor::evm::{utils::pad_and_parse_h160, ERC20Token},
        storage::{
            postgres::{orm, orm::Token},
            Address, StorableToken,
        },
    };

    use crate::storage::{ContractId, StorableProtocolComponent};
    use chrono::Utc;
    use ethers::prelude::{H160, H256};
    use std::str::FromStr;
    use tycho_types::Bytes;

    #[test]
    fn test_storable_token_from_storage() {
        let token_address: Address = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".into();
        let orm_token = Token {
            id: 1,
            account_id: 1,
            symbol: String::from("WETH"),
            decimals: 18,
            tax: 0,
            gas: vec![Some(64), None],
            inserted_ts: Default::default(),
            modified_ts: Default::default(),
            quality: orm::TokenQuality::Normal,
        };
        let contract_id = ContractId::new(Chain::Ethereum, token_address.clone());
        let result = ERC20Token::from_storage(orm_token, contract_id);
        assert!(result.is_ok());

        let token = result.unwrap();
        assert_eq!(token.address, pad_and_parse_h160(&token_address).unwrap());
        assert_eq!(token.symbol, String::from("WETH"));
        assert_eq!(token.decimals, 18);
        assert_eq!(token.gas, vec![Some(64), None]);
    }
    #[test]
    fn test_storable_token_to_storage() {
        let token_address = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".into();
        let erc_token = ERC20Token {
            address: pad_and_parse_h160(&token_address).unwrap(),
            symbol: "WETH".into(),
            decimals: 18,
            tax: 0,
            gas: vec![Some(64), None],
            chain: Chain::Ethereum,
            quality: evm::TokenQuality::Normal,
        };

        let new_token = erc_token.to_storage(22);
        assert_eq!(new_token.account_id, 22);
        assert_eq!(new_token.symbol, erc_token.symbol);
        assert_eq!(new_token.decimals, erc_token.decimals as i32);
        assert_eq!(new_token.gas, vec![Some(64), None]);
    }

    #[test]
    fn test_from_storage_protocol_component() {
        let atts = serde_json::json!({
            "key1": "0xbadbabe1",
            "key2": "0xbadbabe2"
        });

        let val = orm::ProtocolComponent {
            id: 0,
            chain_id: 0,
            external_id: "sample_external_id".to_string(),
            protocol_type_id: 42,
            attributes: Some(atts),
            protocol_system_id: 0,
            created_at: Default::default(),
            deleted_at: None,
            inserted_ts: Default::default(),
            modified_ts: Default::default(),
            creation_tx: 1,
            deletion_tx: None,
        };

        let token_str_addresses = vec![
            "6B175474E89094C44Da98b954EedeAC495271d0F",
            "6B175474E89094C44Da98b954EedeAC495271d0F",
        ];
        let contract_str_addresses = vec![
            "694B89AbC4E1C3002C110D7002e11424aad3d3A5",
            "4665e227c521849a202f808E927d1dc5F63C7941",
        ];

        let tokens: Vec<Address> = token_str_addresses
            .clone()
            .into_iter()
            .map(|t_address| t_address.into())
            .collect();

        let contract_ids: Vec<Address> = contract_str_addresses
            .clone()
            .into_iter()
            .map(|c_address| c_address.into())
            .collect();

        let chain = Chain::Ethereum;
        let protocol_system = "ambient".to_string();

        let protocol_component = evm::ProtocolComponent::from_storage(
            val.clone(),
            &tokens,
            &contract_ids,
            chain,
            &protocol_system,
            H256::from_str("0x88e96d4537bea4d9c05d12549907b32561d3bf31f45aae734cdc119f13406cb6")
                .unwrap(),
        )
        .expect("from_storage failed");

        assert_eq!(protocol_component.id, val.external_id.to_string());
        assert_eq!(protocol_component.protocol_type_name, val.protocol_type_id.to_string());
        assert_eq!(protocol_component.chain, chain);
        assert_eq!(
            protocol_component.tokens,
            token_str_addresses
                .iter()
                .map(|t_address| H160::from_str(t_address).unwrap())
                .collect::<Vec<H160>>()
        );
        assert_eq!(
            protocol_component.contract_ids,
            contract_str_addresses
                .iter()
                .map(|c_address| H160::from_str(c_address).unwrap())
                .collect::<Vec<H160>>()
        );

        let mut expected_attributes = HashMap::new();
        expected_attributes.insert("key1".to_string(), Bytes::from_str("0xbadbabe1").unwrap());
        expected_attributes.insert("key2".to_string(), Bytes::from_str("0xbadbabe2").unwrap());

        assert_eq!(protocol_component.static_attributes, expected_attributes);
    }

    #[test]
    fn test_to_storage_protocol_component() {
        let protocol_component = evm::ProtocolComponent {
            id: "sample_contract_id".to_string(),
            protocol_system: "ambient".to_string(),
            protocol_type_name: "ambient_pool".to_string(),
            chain: Chain::Ethereum,
            tokens: vec![
                H160::from_str("0x6B175474E89094C44Da98b954EedeAC495271d0F").unwrap(),
                H160::from_str("0x6B175474E89094C44Da98b954EedeAC495271d0F").unwrap(),
            ],
            contract_ids: vec![H160::from_low_u64_be(2), H160::from_low_u64_be(3)],
            static_attributes: {
                let mut map = HashMap::new();
                map.insert("key1".to_string(), Bytes::from("badbabe1"));
                map.insert("key2".to_string(), Bytes::from("badbabe2"));
                map
            },
            change: Default::default(),
            creation_tx: H256::from_str(
                "0x88e96d4537bea4d9c05d12549907b32561d3bf31f45aae734cdc119f13406cb6",
            )
            .unwrap(),
            created_at: Default::default(),
        };

        let chain_id = 1;
        let protocol_system_id = 2;
        let protocol_type_id = 42;
        let creation_ts = Utc::now().naive_utc();

        let result = protocol_component.to_storage(
            chain_id,
            protocol_system_id,
            protocol_type_id,
            0,
            creation_ts,
        );

        assert!(result.is_ok());

        let new_protocol_component = result.unwrap();

        assert_eq!(new_protocol_component.external_id, protocol_component.id);
        assert_eq!(new_protocol_component.chain_id, chain_id);

        assert_eq!(new_protocol_component.protocol_type_id, 42);

        assert_eq!(new_protocol_component.protocol_system_id, protocol_system_id);

        let expected_attributes: serde_json::Value =
            serde_json::from_str(r#"{ "key1": "0xbadbabe1", "key2": "0xbadbabe2" }"#).unwrap();

        assert_eq!(new_protocol_component.attributes, Some(expected_attributes));
    }
}
