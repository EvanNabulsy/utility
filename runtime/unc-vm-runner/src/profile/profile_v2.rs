use borsh::{BorshDeserialize, BorshSerialize};
use std::fmt;
use std::ops::Index;
use strum::IntoEnumIterator;
use unc_parameters::{ActionCosts, ExtCosts};
use unc_primitives_core::types::Gas;

/// Deprecated serialization format to store profiles in the database.
///
/// There is no ProfileDataV1 because meta data V1 did no have profiles.
/// Counting thus starts with 2 to match the meta data version numbers.
///
/// This is not part of the protocol but archival nodes still rely on this not
/// changing to answer old tx-status requests with a gas profile.
///
/// It used to store an array that manually mapped `enum Cost` to gas
/// numbers. Now `ProfileDataV2` and `Cost` are deprecated. But to lookup
/// old gas profiles from the DB, we need to keep the code around.
#[derive(Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProfileDataV2 {
    data: DataArray,
}

#[derive(Clone, PartialEq, Eq)]
struct DataArray(Box<[u64; Self::LEN]>);

impl DataArray {
    const LEN: usize = 72;
}

impl ProfileDataV2 {
    pub fn get_ext_cost(&self, ext: ExtCosts) -> u64 {
        self[ext]
    }

    pub fn get_wasm_cost(&self) -> u64 {
        // ProfileV2Cost::WasmInstruction => 62,
        self.data[62]
    }

    fn host_gas(&self) -> u64 {
        ExtCosts::iter().map(|a| self.get_ext_cost(a)).fold(0, u64::saturating_add)
    }

    /// List action cost in the old way, which conflated several action parameters into one.
    ///
    /// This is used to display old gas profiles on the RPC API and in debug output.
    pub fn legacy_action_costs(&self) -> Vec<(&'static str, Gas)> {
        vec![
            ("CREATE_ACCOUNT", self.data[0]),
            ("DELETE_ACCOUNT", self.data[1]),
            ("DEPLOY_CONTRACT", self.data[2]), // contains per byte and base cost
            ("FUNCTION_CALL", self.data[3]),   // contains per byte and base cost
            ("TRANSFER", self.data[4]),
            ("STAKE", self.data[5]),
            ("ADD_KEY", self.data[6]), // contains base + per byte cost for function call keys and full access keys
            ("DELETE_KEY", self.data[7]),
            ("NEW_DATA_RECEIPT_BYTE", self.data[8]), // contains the per-byte cost for sending back a data dependency
            ("NEW_RECEIPT", self.data[9]), // contains base cost for data receipts and action receipts
        ]
    }

    pub fn action_gas(&self) -> u64 {
        self.legacy_action_costs().iter().map(|(_name, cost)| *cost).fold(0, u64::saturating_add)
    }

    /// Test instance with unique numbers in each field.
    pub fn test() -> Self {
        let mut profile_data = ProfileDataV2::default();
        let num_legacy_actions = 10;
        for i in 0..num_legacy_actions {
            profile_data.data.0[i] = i as Gas + 1000;
        }
        for i in num_legacy_actions..DataArray::LEN {
            profile_data.data.0[i] = (i - num_legacy_actions) as Gas;
        }
        profile_data
    }
}

impl fmt::Debug for ProfileDataV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use num_rational::Ratio;
        let host_gas = self.host_gas();
        let action_gas = self.action_gas();

        writeln!(f, "------------------------------")?;
        writeln!(f, "Action gas: {}", action_gas)?;
        writeln!(f, "------ Host functions --------")?;
        for cost in ExtCosts::iter() {
            let d = self.get_ext_cost(cost);
            if d != 0 {
                writeln!(
                    f,
                    "{} -> {} [{}% host]",
                    cost,
                    d,
                    Ratio::new(d * 100, core::cmp::max(host_gas, 1)).to_integer(),
                )?;
            }
        }
        writeln!(f, "------ Actions --------")?;
        for (cost, gas) in self.legacy_action_costs() {
            if gas != 0 {
                writeln!(f, "{} -> {}", cost.to_ascii_lowercase(), gas)?;
            }
        }
        writeln!(f, "------------------------------")?;
        Ok(())
    }
}

impl Index<usize> for DataArray {
    type Output = u64;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl Index<ActionCosts> for ProfileDataV2 {
    type Output = u64;

    fn index(&self, cost: ActionCosts) -> &Self::Output {
        let index = match cost {
            ActionCosts::create_account => 0,
            ActionCosts::delete_account => 1,
            ActionCosts::deploy_contract_base => 2,
            ActionCosts::deploy_contract_byte => 2,
            ActionCosts::function_call_base => 3,
            ActionCosts::function_call_byte => 3,
            ActionCosts::transfer => 4,
            ActionCosts::pledge => 5,
            ActionCosts::add_full_access_key => 6,
            ActionCosts::add_function_call_key_base => 6,
            ActionCosts::add_function_call_key_byte => 6,
            ActionCosts::delete_key => 7,
            ActionCosts::new_data_receipt_byte => 8,
            ActionCosts::new_action_receipt => 9,
            ActionCosts::new_data_receipt_base => 9,
            // new costs added after profile v1 was deprecated don't have this entry
            #[allow(unreachable_patterns)]
            _ => return &0,
        };
        &self.data[index]
    }
}

impl Index<ExtCosts> for ProfileDataV2 {
    type Output = u64;

    fn index(&self, cost: ExtCosts) -> &Self::Output {
        let index = match cost {
            ExtCosts::base => 10,
            ExtCosts::contract_loading_base => 11,
            ExtCosts::contract_loading_bytes => 12,
            ExtCosts::read_memory_base => 13,
            ExtCosts::read_memory_byte => 14,
            ExtCosts::write_memory_base => 15,
            ExtCosts::write_memory_byte => 16,
            ExtCosts::read_register_base => 17,
            ExtCosts::read_register_byte => 18,
            ExtCosts::write_register_base => 19,
            ExtCosts::write_register_byte => 20,
            ExtCosts::utf8_decoding_base => 21,
            ExtCosts::utf8_decoding_byte => 22,
            ExtCosts::utf16_decoding_base => 23,
            ExtCosts::utf16_decoding_byte => 24,
            ExtCosts::sha256_base => 25,
            ExtCosts::sha256_byte => 26,
            ExtCosts::keccak256_base => 27,
            ExtCosts::keccak256_byte => 28,
            ExtCosts::keccak512_base => 29,
            ExtCosts::keccak512_byte => 30,
            ExtCosts::ripemd160_base => 31,
            ExtCosts::ripemd160_block => 32,
            ExtCosts::ecrecover_base => 33,
            ExtCosts::log_base => 34,
            ExtCosts::log_byte => 35,
            ExtCosts::storage_write_base => 36,
            ExtCosts::storage_write_key_byte => 37,
            ExtCosts::storage_write_value_byte => 38,
            ExtCosts::storage_write_evicted_byte => 39,
            ExtCosts::storage_read_base => 40,
            ExtCosts::storage_read_key_byte => 41,
            ExtCosts::storage_read_value_byte => 42,
            ExtCosts::storage_remove_base => 43,
            ExtCosts::storage_remove_key_byte => 44,
            ExtCosts::storage_remove_ret_value_byte => 45,
            ExtCosts::storage_has_key_base => 46,
            ExtCosts::storage_has_key_byte => 47,
            ExtCosts::storage_iter_create_prefix_base => 48,
            ExtCosts::storage_iter_create_prefix_byte => 49,
            ExtCosts::storage_iter_create_range_base => 50,
            ExtCosts::storage_iter_create_from_byte => 51,
            ExtCosts::storage_iter_create_to_byte => 52,
            ExtCosts::storage_iter_next_base => 53,
            ExtCosts::storage_iter_next_key_byte => 54,
            ExtCosts::storage_iter_next_value_byte => 55,
            ExtCosts::touching_trie_node => 56,
            ExtCosts::promise_and_base => 57,
            ExtCosts::promise_and_per_promise => 58,
            ExtCosts::promise_return => 59,
            ExtCosts::validator_pledge_base => 60,
            ExtCosts::validator_total_pledge_base => 61,
            ExtCosts::read_cached_trie_node => 63,
            ExtCosts::alt_bn128_g1_multiexp_base => 64,
            ExtCosts::alt_bn128_g1_multiexp_element => 65,
            ExtCosts::alt_bn128_pairing_check_base => 66,
            ExtCosts::alt_bn128_pairing_check_element => 67,
            ExtCosts::alt_bn128_g1_sum_base => 68,
            ExtCosts::alt_bn128_g1_sum_element => 69,
            ExtCosts::validator_power_base => 60,
            ExtCosts::validator_total_power_base => 61,
            // new costs added after profile v1 was deprecated don't have this entry
            #[allow(unreachable_patterns)]
            _ => return &0,
        };
        &self.data[index]
    }
}

impl BorshDeserialize for DataArray {
    fn deserialize_reader<R: std::io::Read>(rd: &mut R) -> std::io::Result<Self> {
        let data_vec: Vec<u64> = BorshDeserialize::deserialize_reader(rd)?;
        let mut data_array = [0; Self::LEN];
        let len = Self::LEN.min(data_vec.len());
        data_array[0..len].copy_from_slice(&data_vec[0..len]);
        Ok(Self(Box::new(data_array)))
    }
}

impl BorshSerialize for DataArray {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        (&self.0[..]).serialize(writer)
    }
}

impl Default for ProfileDataV2 {
    fn default() -> Self {
        let costs = DataArray(Box::new([0; DataArray::LEN]));
        ProfileDataV2 { data: costs }
    }
}

/// Tests for ProfileDataV2
#[cfg(test)]
mod test {
    use super::*;

    #[test]
    #[cfg(not(feature = "nightly"))]
    fn test_profile_data_debug() {
        let profile_data = ProfileDataV2::test();
        // we don't care about exact formatting, but the numbers should not change unexpectedly
        let pretty_debug_str = format!("{profile_data:#?}");
        expect_test::expect![[r#"
            ------------------------------
            Action gas: 10045
            ------ Host functions --------
            contract_loading_base -> 1 [0% host]
            contract_loading_bytes -> 2 [0% host]
            read_memory_base -> 3 [0% host]
            read_memory_byte -> 4 [0% host]
            write_memory_base -> 5 [0% host]
            write_memory_byte -> 6 [0% host]
            read_register_base -> 7 [0% host]
            read_register_byte -> 8 [0% host]
            write_register_base -> 9 [0% host]
            write_register_byte -> 10 [0% host]
            utf8_decoding_base -> 11 [0% host]
            utf8_decoding_byte -> 12 [0% host]
            utf16_decoding_base -> 13 [0% host]
            utf16_decoding_byte -> 14 [0% host]
            sha256_base -> 15 [0% host]
            sha256_byte -> 16 [0% host]
            keccak256_base -> 17 [0% host]
            keccak256_byte -> 18 [1% host]
            keccak512_base -> 19 [1% host]
            keccak512_byte -> 20 [1% host]
            ripemd160_base -> 21 [1% host]
            ripemd160_block -> 22 [1% host]
            ecrecover_base -> 23 [1% host]
            log_base -> 24 [1% host]
            log_byte -> 25 [1% host]
            storage_write_base -> 26 [1% host]
            storage_write_key_byte -> 27 [1% host]
            storage_write_value_byte -> 28 [1% host]
            storage_write_evicted_byte -> 29 [1% host]
            storage_read_base -> 30 [1% host]
            storage_read_key_byte -> 31 [1% host]
            storage_read_value_byte -> 32 [1% host]
            storage_remove_base -> 33 [1% host]
            storage_remove_key_byte -> 34 [1% host]
            storage_remove_ret_value_byte -> 35 [2% host]
            storage_has_key_base -> 36 [2% host]
            storage_has_key_byte -> 37 [2% host]
            storage_iter_create_prefix_base -> 38 [2% host]
            storage_iter_create_prefix_byte -> 39 [2% host]
            storage_iter_create_range_base -> 40 [2% host]
            storage_iter_create_from_byte -> 41 [2% host]
            storage_iter_create_to_byte -> 42 [2% host]
            storage_iter_next_base -> 43 [2% host]
            storage_iter_next_key_byte -> 44 [2% host]
            storage_iter_next_value_byte -> 45 [2% host]
            touching_trie_node -> 46 [2% host]
            read_cached_trie_node -> 53 [3% host]
            promise_and_base -> 47 [2% host]
            promise_and_per_promise -> 48 [2% host]
            promise_return -> 49 [2% host]
            validator_pledge_base -> 50 [2% host]
            validator_total_pledge_base -> 51 [2% host]
            alt_bn128_g1_multiexp_base -> 54 [3% host]
            alt_bn128_g1_multiexp_element -> 55 [3% host]
            alt_bn128_pairing_check_base -> 56 [3% host]
            alt_bn128_pairing_check_element -> 57 [3% host]
            alt_bn128_g1_sum_base -> 58 [3% host]
            alt_bn128_g1_sum_element -> 59 [3% host]
            ------ Actions --------
            create_account -> 1000
            delete_account -> 1001
            deploy_contract -> 1002
            function_call -> 1003
            transfer -> 1004
            pledge -> 1005
            add_key -> 1006
            delete_key -> 1007
            new_data_receipt_byte -> 1008
            new_receipt -> 1009
            ------------------------------
        "#]]
        .assert_eq(&pretty_debug_str)
    }

    #[test]
    fn test_profile_data_debug_no_data() {
        let profile_data = ProfileDataV2::default();
        // we don't care about exact formatting, but at least it should not panic
        println!("{:#?}", &profile_data);
    }
}
