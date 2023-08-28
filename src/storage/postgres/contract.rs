use std::collections::{hash_map::Entry, HashMap};

use chrono::{NaiveDateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use ethers::types::{H160, U256};

use crate::{
    models::Chain,
    storage::{orm, schema, BlockIdentifier, BlockOrTimestamp, ContractId, StorageError},
};

use super::PostgresGateway;

impl<B, TX> PostgresGateway<B, TX> {
    async fn get_slots_delta(
        &self,
        chain: Chain,
        start_version: Option<BlockOrTimestamp>,
        target_version: Option<BlockOrTimestamp>,
        conn: &mut AsyncPgConnection,
    ) -> Result<HashMap<H160, HashMap<U256, U256>>, StorageError> {
        let chain_id = self.get_chain_id(chain);
        // To support blocks as versions, we need to ingest all blocks, else the
        // below method can error for any blocks that are not present.
        let start_version_ts = version_to_ts(&start_version, conn).await?;
        let target_version_ts = version_to_ts(&target_version, conn).await?;

        let changed_values = if start_version_ts <= target_version_ts {
            // Going forward
            //                  ]     relevant changes     ]
            // -----------------|--------------------------|
            //                start                     target
            // We query for changes between start and target version. Then sort
            // these by contract and slot by change time in a desending matter
            // (latest change first). Next we deduplicate by contract and slot.
            // Finally we select the value column to give us the latest value
            // within the version range.
            schema::contract_storage::table
                .inner_join(schema::contract::table.inner_join(schema::chain::table))
                .filter(schema::chain::id.eq(chain_id))
                .filter(schema::contract_storage::valid_from.gt(start_version_ts))
                .filter(schema::contract_storage::valid_from.le(target_version_ts))
                .order_by((
                    schema::contract::id,
                    schema::contract_storage::slot,
                    schema::contract_storage::valid_from.desc(),
                    schema::contract_storage::ordinal.desc(),
                ))
                .select((
                    schema::contract::id,
                    schema::contract_storage::slot,
                    schema::contract_storage::value,
                ))
                .distinct_on((schema::contract::id, schema::contract_storage::slot))
                .get_results::<(i64, Vec<u8>, Option<Vec<u8>>)>(conn)
                .await
                .unwrap()
        } else {
            // Going backwards
            //                  ]     relevant changes     ]
            // -----------------|--------------------------|
            //                target                     start
            // We query for changes between target and start version. Then sort
            // these for each contract and slot by change time in an ascending
            // manner. Next, we deduplicate by taking the first row for each
            // contract and slot. Finally we select the previous_value column to
            // give us the value before this first change within the version
            // range.
            schema::contract_storage::table
                .inner_join(schema::contract::table.inner_join(schema::chain::table))
                .filter(schema::chain::id.eq(chain_id))
                .filter(schema::contract_storage::valid_from.gt(target_version_ts))
                .filter(schema::contract_storage::valid_from.le(start_version_ts))
                .order_by((
                    schema::contract::id.asc(),
                    schema::contract_storage::slot.asc(),
                    schema::contract_storage::valid_from.asc(),
                    schema::contract_storage::ordinal.asc(),
                ))
                .select((
                    schema::contract::id,
                    schema::contract_storage::slot,
                    schema::contract_storage::previous_value,
                ))
                .distinct_on((schema::contract::id, schema::contract_storage::slot))
                .get_results::<(i64, Vec<u8>, Option<Vec<u8>>)>(conn)
                .await
                .unwrap()
        };

        // We retrieve contract addresses separately because this is more
        // efficient for the most common cases. In the most common case, only a
        // handful of contracts that we are interested in will have had changes
        // that need to be reverted. The previous query only returns duplicated
        // contract ids, which are lighweight (8 byte vs 20 for addresses), once
        // deduplicated we only fetch the associated addresses. These addresses
        // are considered immutable so if necessary we could event cache these
        // locally.
        // In the worst case each changed slot is only changed on a different
        // contract. On mainnet that would be at max 300 contracts/slots, which
        // although not ideal is still bearable.
        let contract_addresses = schema::contract::table
            .filter(schema::contract::id.eq_any(changed_values.iter().map(|(cid, _, _)| cid)))
            .select((schema::contract::id, schema::contract::address))
            .get_results::<(i64, Vec<u8>)>(conn)
            .await
            .map_err(StorageError::from)?
            .iter()
            .map(|(k, v)| {
                if v.len() != 20 {
                    return Err(StorageError::DecodeError(format!(
                        "Invalid contract address found for contract with id: {}, address: {}",
                        k,
                        hex::encode(v)
                    )));
                }
                Ok((*k, H160::from_slice(v)))
            })
            .collect::<Result<HashMap<i64, H160>, StorageError>>()?;

        let mut result: HashMap<H160, HashMap<U256, U256>> =
            HashMap::with_capacity(contract_addresses.len());
        for (cid, raw_key, raw_val) in changed_values.into_iter() {
            // note this can theoretically happen (only if there is some really
            // bad database inconsistency) because the call above simply filters
            // for contracts ids, but won't error or give any inidication of a
            // missing contract id.
            let contract_address = contract_addresses.get(&cid).ok_or_else(|| {
                StorageError::DecodeError(format!("Failed to find contract address for id {}", cid))
            })?;

            if raw_key.len() != 32 {
                return Err(StorageError::DecodeError(format!(
                    "Invalid byte length for U256 in slot key! Found: 0x{}",
                    hex::encode(raw_key)
                )));
            }
            let v = if let Some(val) = raw_val {
                if val.len() != 32 {
                    return Err(StorageError::DecodeError(format!(
                        "Invalid byte length for U256 in slot value! Found: 0x{}",
                        hex::encode(val)
                    )));
                }
                U256::from_big_endian(&val)
            } else {
                U256::zero()
            };

            let k = U256::from_big_endian(&raw_key);

            match result.entry(*contract_address) {
                Entry::Occupied(mut e) => {
                    e.get_mut().insert(k, v);
                }
                Entry::Vacant(e) => {
                    let mut contract_storage = HashMap::new();
                    contract_storage.insert(k, v);
                    e.insert(contract_storage);
                }
            }
        }
        Ok(result)
    }
}

/// Given a version find the corresponding timestamp.
///
/// If the version is a block, it will query the database for that block and
/// return its timestamp.
///
/// ## Note:
/// This can fail if there is no block present in the db. With the current table
/// schema this means, that there were no changes detected at that block, but
/// there might have been on previous or in later blocks.
async fn version_to_ts(
    start_version: &Option<BlockOrTimestamp>,
    conn: &mut AsyncPgConnection,
) -> Result<NaiveDateTime, StorageError> {
    match &start_version {
        Some(BlockOrTimestamp::Block(BlockIdentifier::Hash(h))) => {
            Ok(orm::Block::by_hash(&h, conn)
                .await
                .map_err(|err| StorageError::from_diesel(err, "Block", &hex::encode(h), None))?
                .ts)
        }
        Some(BlockOrTimestamp::Block(BlockIdentifier::Number((chain, no)))) => {
            Ok(orm::Block::by_number(*chain, *no, conn)
                .await
                .map_err(|err| StorageError::from_diesel(err, "Block", &format!("{}", no), None))?
                .ts)
        }
        Some(BlockOrTimestamp::Timestamp(ts)) => Ok(*ts),
        None => Ok(Utc::now().naive_utc()),
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use diesel_async::AsyncConnection;

    use crate::{extractor::evm, storage::postgres::fixtures};

    use super::*;

    async fn setup_db() -> AsyncPgConnection {
        let db_url = std::env::var("DATABASE_URL").unwrap();
        let mut conn = AsyncPgConnection::establish(&db_url).await.unwrap();
        conn.begin_test_transaction().await.unwrap();
        conn
    }

    async fn setup_slots_delta(conn: &mut AsyncPgConnection) {
        let chain_id = fixtures::insert_chain(conn, "ethereum").await;
        let blk = fixtures::insert_blocks(conn, chain_id).await;
        let txn = fixtures::insert_txns(
            conn,
            &[
                (
                    blk[0],
                    1i64,
                    "0xbb7e16d797a9e2fbc537e30f91ed3d27a254dd9578aa4c3af3e5f0d3e8130945",
                ),
                (
                    blk[1],
                    1i64,
                    "0x3108322284d0a89a7accb288d1a94384d499504fe7e04441b0706c7628dee7b7",
                ),
            ],
        )
        .await;
        let c0 = fixtures::insert_contract(
            conn,
            "0x6B175474E89094C44Da98b954EedeAC495271d0F",
            "c0",
            chain_id,
        )
        .await;
        fixtures::insert_slots(
            conn,
            c0,
            txn[0],
            "2020-01-01T00:00:00",
            &[(0, 1), (1, 5), (2, 1)],
        )
        .await;
        fixtures::insert_slots(
            conn,
            c0,
            txn[1],
            "2020-01-01T01:00:00",
            &[(0, 2), (1, 3), (5, 25), (6, 30)],
        )
        .await;
    }

    async fn print_slots(conn: &mut AsyncPgConnection) {
        let all_slots: Vec<orm::ContractStorage> = schema::contract_storage::table
            .select(orm::ContractStorage::as_select())
            .get_results(conn)
            .await
            .unwrap();

        dbg!(all_slots
            .iter()
            .map(|s| (
                s.contract_id,
                U256::from_big_endian(&s.slot),
                s.previous_value
                    .clone()
                    .map(|v| U256::from_big_endian(&v))
                    .unwrap_or_else(U256::zero),
                s.value
                    .clone()
                    .map(|v| U256::from_big_endian(&v))
                    .unwrap_or_else(U256::zero),
                s.valid_from,
                s.valid_to
            ))
            .collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn get_slots_delta_forward() {
        let mut conn = setup_db().await;
        setup_slots_delta(&mut conn).await;
        let gw = PostgresGateway::<evm::Block, evm::Transaction>::from_connection(&mut conn).await;
        let storage: HashMap<U256, U256> = vec![(0, 2), (1, 3), (5, 25), (6, 30)]
            .iter()
            .map(|(k, v)| (U256::from(*k), U256::from(*v)))
            .collect();
        let mut exp = HashMap::new();
        let addr = H160::from_str("0x6B175474E89094C44Da98b954EedeAC495271d0F").unwrap();
        exp.insert(addr, storage);

        let res = gw
            .get_slots_delta(
                Chain::Ethereum,
                Some(BlockOrTimestamp::Timestamp(
                    "2020-01-01T00:00:00".parse::<NaiveDateTime>().unwrap(),
                )),
                Some(BlockOrTimestamp::Timestamp(
                    "2020-01-01T02:00:00".parse::<NaiveDateTime>().unwrap(),
                )),
                &mut conn,
            )
            .await
            .unwrap();

        assert_eq!(res, exp);
    }

    #[tokio::test]
    async fn get_slots_delta_backward() {
        let mut conn = setup_db().await;
        setup_slots_delta(&mut conn).await;
        let gw = PostgresGateway::<evm::Block, evm::Transaction>::from_connection(&mut conn).await;
        let storage: HashMap<U256, U256> = vec![(0, 1), (1, 5), (5, 0), (6, 0)]
            .iter()
            .map(|(k, v)| (U256::from(*k), U256::from(*v)))
            .collect();
        let mut exp = HashMap::new();
        let addr = H160::from_str("0x6B175474E89094C44Da98b954EedeAC495271d0F").unwrap();
        exp.insert(addr, storage);

        let res = gw
            .get_slots_delta(
                Chain::Ethereum,
                Some(BlockOrTimestamp::Timestamp(
                    "2020-01-01T02:00:00".parse::<NaiveDateTime>().unwrap(),
                )),
                Some(BlockOrTimestamp::Timestamp(
                    "2020-01-01T00:00:00".parse::<NaiveDateTime>().unwrap(),
                )),
                &mut conn,
            )
            .await
            .unwrap();

        assert_eq!(res, exp);
    }
}
