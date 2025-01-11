mod pb;
use hex;
use num_bigint::BigUint;
use std::convert::TryInto;
use substreams::Hex;
use serde::{Deserialize, Serialize, Serializer, Deserializer};
use serde::de::{self, Visitor};
use std::fmt;
use substreams_ethereum::pb::eth::v2::block::DetailLevel;
use substreams_ethereum::pb::eth::v2::Block;
use substreams_ethereum::pb::eth::v2::Call;
use substreams_ethereum::pb::eth::v2::StorageChange;
use pb::pool_price_changes::v1::Slot0Change;
use pb::pool_price_changes::v1 as pool_price_changes;

#[allow(unused_imports)]
use num_traits::cast::ToPrimitive;

/*
    Definition of StorageChange in protobuf:

message StorageChange {
  /// 20 byte. For uniswap v3, this is the contract address
  bytes address = 1;

  /// the storage slot.
  /// "0000000000000000000000000000000000000000000000000000000000000000" is slot0
  /// (i.e., the first 32 byte)
  bytes key = 2;

  bytes old_value = 3;
  bytes new_value = 4;

  /// The block's global ordinal when the storage change was recorded, refer to [Block]
  /// documentation for further information about ordinals and total ordering.
  uint64 ordinal = 5;
}
*/

#[derive(Debug, Serialize, Deserialize)]
struct StorageChangeWrapper {
    address: String,
    key: String,

    #[serde(serialize_with = "vec_to_hex", deserialize_with = "vec_from_hex")]
    old_value: Vec<u8>,

    #[serde(serialize_with = "vec_to_hex", deserialize_with = "vec_from_hex")]
    new_value: Vec<u8>,

    ordinal: u64,
}

/// Serialize a Vec<u8> as a hex string
fn vec_to_hex<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let hex_string = hex::encode(bytes);
    serializer.serialize_str(&hex_string)
}

/// Deserialize a Vec<u8> from a hex string
fn vec_from_hex<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    struct HexVisitor;

    impl<'de> Visitor<'de> for HexVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a hex string representing bytes")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            hex::decode(v).map_err(de::Error::custom)
        }
    }
    deserializer.deserialize_str(HexVisitor)
}

impl From<&StorageChange> for StorageChangeWrapper {
    fn from(change: &StorageChange) -> Self {
        StorageChangeWrapper {
            address: hex::encode(&change.address),
            key: hex::encode(&change.key),
            old_value: change.old_value.clone(),
            new_value: change.new_value.clone(),
            ordinal: change.ordinal,
        }
    }
}

/*
struct Slot0 {
    uint160 sqrtPriceX96;
    int24 tick;
    uint16 observationIndex;
    uint16 observationCardinality;
    uint16 observationCardinalityNext;
    uint8 feeProtocol;
    bool unlocked;
}
*/
fn decode_slot0(slot0_bytes: &Vec<u8>) -> (BigUint, i32) {
    if slot0_bytes.len() != 32 {
        panic!("Slot0 must be a 32-byte array");
    }
    let sqrt_bytes: Vec<u8> = slot0_bytes[12..].iter().cloned().collect();
    let sqrt_price: BigUint = BigUint::from_bytes_be(&sqrt_bytes);

    let tick_bytes: Vec<u8> = slot0_bytes[9..12].iter().cloned().collect();
    let tick_biguint = BigUint::from_bytes_be(&tick_bytes);
    let tick: i32 = tick_biguint.try_into().unwrap();

    (sqrt_price, tick)
}

#[derive(Serialize)]
struct StorageChangeSlot0Decoded {
    address: String,
    ordinal: u64,
    sqrt_price: String,
    tick: i32,
}

impl From<&StorageChangeWrapper> for StorageChangeSlot0Decoded {
    fn from(change: &StorageChangeWrapper) -> Self {
        let (sqrt_price, tick) = decode_slot0(&change.new_value);
        StorageChangeSlot0Decoded {
            address: change.address.clone(),
            ordinal: change.ordinal,
            sqrt_price: sqrt_price.to_string(),
            tick: tick,
        }
    }
}

fn is_call_contain_target_contract(call: &Call, uniswap_v3_pool_address: &str) -> bool {
    let storage_changes: &Vec<StorageChange> = &call.storage_changes;

    let mut contained = false;

    for sc in storage_changes.into_iter() {
        let scw: StorageChangeWrapper = sc.into();
        if scw.address == uniswap_v3_pool_address {
            contained = true;
            break;
        }
    }

    contained
}

substreams_ethereum::init!();

#[substreams::handlers::map]
fn map_pool_price_changes(blk: Block) -> pool_price_changes::PoolPriceChagnes {
    let mut price_changes = pool_price_changes::PoolPriceChagnes::default();

    // Define the Uniswap V3 pool address you want to track
    // contract address of CBETH-ETH.1bp: 0x840deeef2f115cf50da625f7368c24af6fe74410
    let uniswap_v3_pool_address = "840deeef2f115cf50da625f7368c24af6fe74410";
    let slot0_storage_address = "0000000000000000000000000000000000000000000000000000000000000000";

    // check detail_level of Block
    // Only EXTENDED level would contain storage changes of transactions
    let detail_level = blk.detail_level();
    if detail_level != DetailLevel::DetaillevelExtended {
        price_changes.extra = String::from("block detail level");
        return price_changes;
    }

    // Iterate over successful transactions
    // (transactions() 's return values are 'successful' trxs)
    for transaction_trace in blk.transactions().into_iter() {
        let transaction_hash_str: String = hex::encode(&transaction_trace.hash);

        let mut slot0_change: Option<Slot0Change> = None; // Define an optional Slot0Change

        let calls: &Vec<Call> = &transaction_trace.calls;
        for call in calls.into_iter() {
            if !is_call_contain_target_contract(call, uniswap_v3_pool_address) {
                continue;
            }

            let storage_changes: &Vec<StorageChange> = &call.storage_changes;

            let wrapped_changes: Vec<StorageChangeWrapper> = storage_changes
                .iter()
                .map(|change: &StorageChange| change.into())
                .filter(|change_wrapper: &StorageChangeWrapper| {
                    change_wrapper.address == uniswap_v3_pool_address
                })
                .filter(|change_wrapper: &StorageChangeWrapper| {
                    change_wrapper.key == slot0_storage_address
                })
                .collect();

            // Take the StorageChange with that largest ordinal,
            // which is the one in effect at last.
            if let Some(sc) = wrapped_changes.iter().max_by_key(|wrapper| wrapper.ordinal) {
                let scd: StorageChangeSlot0Decoded = sc.into();
                // let sc_json: String = serde_json::to_string(&sc).unwrap();
                // let scd_json: String = serde_json::to_string(&scd).unwrap();
                let s0_change = Slot0Change {
                    transaction_hash: transaction_hash_str.clone(),
                    sqrt_price_x96: scd.sqrt_price,
                    tick: scd.tick,
                    // storage_change_json: sc_json,
                    // storage_change_decoded_json: scd_json,
                    storage_change_json: String::default(),
                    storage_change_decoded_json: String::default(),
                };
                slot0_change = Some(s0_change);
            }
        }

        match slot0_change {
            Some(change) => {
                price_changes.slot0_changes.push(change);
            },
            None => (),
        }
    }

    // If no transaction in this block change the target pool's price,
    // (i.e., slot0_changes.len() == 0), emit a warning msg.
    if price_changes.slot0_changes.len() == 0{
        price_changes.extra = String::from("No matching transaction for pool");
    }

    price_changes.block_hash = Hex(&blk.hash).to_string();
    price_changes.block_number = blk.number.to_u64().unwrap();
    price_changes.block_timestamp = Some(blk.timestamp().to_owned());
    price_changes.num_transactions = blk.transaction_traces.len() as u64;
    
    price_changes
}
