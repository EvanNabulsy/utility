use std::sync::Arc;

use crate::EpochManagerAdapter;
use unc_primitives::hash::CryptoHash;
use unc_primitives::types::{AccountId, ShardId};

/// TrackedConfig::AllShards: track all shards
#[derive(Clone)]
pub struct ShardTracker {
    epoch_manager: Arc<dyn EpochManagerAdapter>,
}

impl ShardTracker {
    pub fn new(epoch_manager: Arc<dyn EpochManagerAdapter>) -> Self {
        ShardTracker { epoch_manager }
    }

    pub fn new_empty(epoch_manager: Arc<dyn EpochManagerAdapter>) -> Self {
        Self::new(epoch_manager)
    }

    /// Whether the client cares about some shard right now.
    /// * If `account_id` is None, `is_me` is not checked and the
    /// result indicates whether the client is tracking the shard
    /// * If `account_id` is not None, it is supposed to be a validator
    /// account and `is_me` indicates whether we check what shards
    /// the client tracks.
    pub fn care_about_shard(
        &self,
        account_id: Option<&AccountId>,
        parent_hash: &CryptoHash,
        shard_id: ShardId,
        is_me: bool,
    ) -> bool {
        // TODO: fix these unwrap_or here and handle error correctly. The current behavior masks potential errors and bugs
        // https://github.com/utnet-org/utility/issues/4936
        if let Some(account_id) = account_id {
            let account_cares_about_shard = self
                .epoch_manager
                .cares_about_shard_from_prev_block(parent_hash, account_id, shard_id)
                .unwrap_or(false);
            if account_cares_about_shard {
                // An account has to track this shard because of its validation duties.
                return true;
            }
            if !is_me {
                // We don't know how another node is configured.
                // It may track all shards, it may track no additional shards.
                return false;
            } else {
                // We have access to the node config. Use the config to find a definite answer.
            }
        }
        true
    }

    /// Whether the client cares about some shard in the next epoch.
    ///  Note that `shard_id` always refers to a shard in the current epoch
    ///  If shard layout will change next epoch,
    ///  returns true if it cares about any shard that `shard_id` will split to
    /// * If `account_id` is None, `is_me` is not checked and the
    /// result indicates whether the client will track the shard
    /// * If `account_id` is not None, it is supposed to be a validator
    /// account and `is_me` indicates whether we check what shards
    /// the client will track.
    pub fn will_care_about_shard(
        &self,
        account_id: Option<&AccountId>,
        parent_hash: &CryptoHash,
        shard_id: ShardId,
        is_me: bool,
    ) -> bool {
        if let Some(account_id) = account_id {
            let account_cares_about_shard = {
                self.epoch_manager
                    .cares_about_shard_next_epoch_from_prev_block(parent_hash, account_id, shard_id)
                    .unwrap_or(false)
            };
            if account_cares_about_shard {
                // An account has to track this shard because of its validation duties.
                return true;
            }
            if !is_me {
                // We don't know how another node is configured.
                // It may track all shards, it may track no additional shards.
                return false;
            } else {
                // We have access to the node config. Use the config to find a definite answer.
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::ShardTracker;
    use crate::test_utils::hash_range;
    use crate::{EpochManager, EpochManagerAdapter, EpochManagerHandle, RewardCalculator};
    use num_rational::Ratio;
    use std::collections::HashSet;
    use std::sync::Arc;
    use unc_crypto::{KeyType, PublicKey};
    use unc_primitives::epoch_manager::block_info::BlockInfo;
    use unc_primitives::epoch_manager::{AllEpochConfig, EpochConfig};
    use unc_primitives::hash::CryptoHash;
    use unc_primitives::shard_layout::ShardLayout;
    use unc_primitives::types::validator_power::ValidatorPower;
    use unc_primitives::types::validator_stake::ValidatorPledge;
    use unc_primitives::types::{BlockHeight, EpochId, NumShards, ProtocolVersion, ShardId};
    use unc_primitives::version::ProtocolFeature::SimpleNightshade;
    use unc_primitives::version::PROTOCOL_VERSION;
    use unc_store::test_utils::create_test_store;

    const DEFAULT_TOTAL_SUPPLY: u128 = 1_000_000_000_000;

    fn get_epoch_manager(
        genesis_protocol_version: ProtocolVersion,
        num_shards: NumShards,
        use_production_config: bool,
    ) -> EpochManagerHandle {
        let store = create_test_store();
        let initial_epoch_config = EpochConfig {
            epoch_length: 1,
            num_block_producer_seats: 1,
            num_block_producer_seats_per_shard: vec![1],
            avg_hidden_validator_seats_per_shard: vec![],
            block_producer_kickout_threshold: 90,
            chunk_producer_kickout_threshold: 60,
            fishermen_threshold: 0,
            online_max_threshold: Ratio::from_integer(1),
            online_min_threshold: Ratio::new(90, 100),
            minimum_pledge_divisor: 1,
            protocol_upgrade_pledge_threshold: Ratio::new(80, 100),
            shard_layout: ShardLayout::v0(num_shards, 0),
            validator_selection_config: Default::default(),
            validator_max_kickout_pledge_perc: 100,
        };
        let reward_calculator = RewardCalculator {
            max_inflation_rate: Ratio::from_integer(0),
            num_blocks_per_year: 1000000,
            epoch_length: 1,
            protocol_reward_rate: Ratio::from_integer(0),
            protocol_treasury_account: "test".parse().unwrap(),
            online_max_threshold: initial_epoch_config.online_max_threshold,
            online_min_threshold: initial_epoch_config.online_min_threshold,
            num_seconds_per_year: 1000000,
        };
        EpochManager::new(
            store,
            AllEpochConfig::new(use_production_config, initial_epoch_config, "test-chain"),
            genesis_protocol_version,
            reward_calculator,
            vec![ValidatorPower::new(
                "test".parse().unwrap(),
                PublicKey::empty(KeyType::ED25519),
                100,
            )],
        )
        .unwrap()
        .into_handle()
    }

    pub fn record_block(
        epoch_manager: &mut EpochManager,
        prev_h: CryptoHash,
        cur_h: CryptoHash,
        height: BlockHeight,
        power_proposals: Vec<ValidatorPower>,
        pledge_proposals: Vec<ValidatorPledge>,
        protocol_version: ProtocolVersion,
    ) {
        epoch_manager
            .record_block_info(
                BlockInfo::new(
                    cur_h,
                    height,
                    0,
                    prev_h,
                    prev_h,
                    power_proposals,
                    pledge_proposals,
                    vec![],
                    vec![],
                    DEFAULT_TOTAL_SUPPLY,
                    protocol_version,
                    height * 10u64.pow(9),
                ),
                [0; 32],
            )
            .unwrap()
            .commit()
            .unwrap();
    }

    fn get_all_shards_care_about(
        tracker: &ShardTracker,
        shard_ids: &[ShardId],
        parent_hash: &CryptoHash,
    ) -> HashSet<ShardId> {
        shard_ids
            .into_iter()
            .filter(|&&shard_id| tracker.care_about_shard(None, parent_hash, shard_id, true))
            .cloned()
            .collect()
    }

    fn get_all_shards_will_care_about(
        tracker: &ShardTracker,
        shard_ids: &[ShardId],
        parent_hash: &CryptoHash,
    ) -> HashSet<ShardId> {
        shard_ids
            .into_iter()
            .filter(|&&shard_id| tracker.will_care_about_shard(None, parent_hash, shard_id, true))
            .cloned()
            .collect()
    }

    #[test]
    fn test_track_accounts() {
        let shard_ids: Vec<_> = (0..4).collect();
        let epoch_manager =
            get_epoch_manager(PROTOCOL_VERSION, shard_ids.len() as NumShards, false);
        let shard_layout = epoch_manager.read().get_shard_layout(&EpochId::default()).unwrap();
        let tracked_accounts = vec!["test1".parse().unwrap(), "test2".parse().unwrap()];
        let tracker =
            ShardTracker::new(TrackedConfig::Accounts(tracked_accounts), Arc::new(epoch_manager));
        let mut total_tracked_shards = HashSet::new();
        total_tracked_shards
            .insert(account_id_to_shard_id(&"test1".parse().unwrap(), &shard_layout));
        total_tracked_shards
            .insert(account_id_to_shard_id(&"test2".parse().unwrap(), &shard_layout));

        assert_eq!(
            get_all_shards_care_about(&tracker, &shard_ids, &CryptoHash::default()),
            total_tracked_shards
        );
        assert_eq!(
            get_all_shards_will_care_about(&tracker, &shard_ids, &CryptoHash::default()),
            total_tracked_shards
        );
    }

    #[test]
    fn test_track_all_shards() {
        let shard_ids: Vec<_> = (0..4).collect();
        let epoch_manager =
            get_epoch_manager(PROTOCOL_VERSION, shard_ids.len() as NumShards, false);
        let tracker = ShardTracker::new(TrackedConfig::AllShards, Arc::new(epoch_manager));
        let total_tracked_shards: HashSet<_> = shard_ids.iter().cloned().collect();

        assert_eq!(
            get_all_shards_care_about(&tracker, &shard_ids, &CryptoHash::default()),
            total_tracked_shards
        );
        assert_eq!(
            get_all_shards_will_care_about(&tracker, &shard_ids, &CryptoHash::default()),
            total_tracked_shards
        );
    }

    #[test]
    fn test_track_schedule() {
        // Creates a ShardTracker that changes every epoch tracked shards.
        let shard_ids: Vec<_> = (0..4).collect();
        let epoch_manager =
            Arc::new(get_epoch_manager(PROTOCOL_VERSION, shard_ids.len() as NumShards, false));
        let subset1 = HashSet::from([0, 1]);
        let subset2 = HashSet::from([1, 2]);
        let subset3 = HashSet::from([2, 3]);
        let tracker = ShardTracker::new(
            TrackedConfig::Schedule(vec![
                subset1.clone().into_iter().collect(),
                subset2.clone().into_iter().collect(),
                subset3.clone().into_iter().collect(),
            ]),
            epoch_manager.clone(),
        );

        let h = hash_range(8);
        {
            let mut epoch_manager = epoch_manager.write();
            for i in 0..8 {
                record_block(
                    &mut epoch_manager,
                    if i > 0 { h[i - 1] } else { CryptoHash::default() },
                    h[i],
                    i as u64,
                    vec![],
                    PROTOCOL_VERSION,
                );
            }
        }

        assert_eq!(get_all_shards_care_about(&tracker, &shard_ids, &h[4]), subset2);
        assert_eq!(get_all_shards_care_about(&tracker, &shard_ids, &h[5]), subset3);
        assert_eq!(get_all_shards_care_about(&tracker, &shard_ids, &h[6]), subset1);
        assert_eq!(get_all_shards_care_about(&tracker, &shard_ids, &h[7]), subset2);

        assert_eq!(get_all_shards_will_care_about(&tracker, &shard_ids, &h[4]), subset3);
        assert_eq!(get_all_shards_will_care_about(&tracker, &shard_ids, &h[5]), subset1);
        assert_eq!(get_all_shards_will_care_about(&tracker, &shard_ids, &h[6]), subset2);
        assert_eq!(get_all_shards_will_care_about(&tracker, &shard_ids, &h[7]), subset3);
    }

    #[test]
    fn test_track_shards_shard_layout_change() {
        let simple_nightshade_version = SimpleNightshade.protocol_version();
        let epoch_manager = get_epoch_manager(simple_nightshade_version - 1, 1, true);
        let tracked_accounts =
            vec!["a.unc".parse().unwrap(), "unc".parse().unwrap(), "zoo".parse().unwrap()];
        let tracker = ShardTracker::new(
            TrackedConfig::Accounts(tracked_accounts.clone()),
            Arc::new(epoch_manager.clone()),
        );

        let h = hash_range(8);
        {
            let mut epoch_manager = epoch_manager.write();
            record_block(
                &mut epoch_manager,
                CryptoHash::default(),
                h[0],
                0,
                vec![],
                simple_nightshade_version,
            );
            for i in 1..8 {
                record_block(
                    &mut epoch_manager,
                    h[i - 1],
                    h[i],
                    i as u64,
                    vec![],
                    simple_nightshade_version,
                );
            }
            assert_eq!(
                epoch_manager.get_epoch_info(&EpochId(h[0])).unwrap().protocol_version(),
                simple_nightshade_version - 1
            );
            assert_eq!(
                epoch_manager.get_epoch_info(&EpochId(h[1])).unwrap().protocol_version(),
                simple_nightshade_version
            );
        }

        // verify tracker is tracking the correct shards before and after resharding
        for i in 1..8 {
            let mut total_next_tracked_shards = HashSet::new();
            let next_epoch_id = epoch_manager.get_next_epoch_id_from_prev_block(&h[i - 1]).unwrap();
            let next_shard_layout = epoch_manager.get_shard_layout(&next_epoch_id).unwrap();

            let mut total_tracked_shards = HashSet::new();
            let epoch_id = epoch_manager.get_epoch_id_from_prev_block(&h[i - 1]).unwrap();
            let shard_layout = epoch_manager.get_shard_layout(&epoch_id).unwrap();

            for account_id in tracked_accounts.iter() {
                let shard_id = account_id_to_shard_id(account_id, &shard_layout);
                total_tracked_shards.insert(shard_id);

                let next_shard_id = account_id_to_shard_id(account_id, &next_shard_layout);
                total_next_tracked_shards.insert(next_shard_id);
            }

            assert_eq!(
                get_all_shards_care_about(
                    &tracker,
                    &shard_layout.shard_ids().collect::<Vec<_>>(),
                    &h[i - 1]
                ),
                total_tracked_shards
            );
            assert_eq!(
                get_all_shards_will_care_about(
                    &tracker,
                    &next_shard_layout.shard_ids().collect::<Vec<_>>(),
                    &h[i - 1]
                ),
                total_next_tracked_shards
            );
        }
    }
}
