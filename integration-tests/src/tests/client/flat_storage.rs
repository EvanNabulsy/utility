/// Tests which check correctness of background flat storage creation.
use assert_matches::assert_matches;
use framework::config::GenesisExt;
use framework::test_utils::TestEnvNightshadeSetupExt;
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use unc_chain::{ChainGenesis, Provenance};
use unc_chain_configs::Genesis;
use unc_client::test_utils::TestEnv;
use unc_client::ProcessTxResponse;
use unc_crypto::{InMemorySigner, KeyType, Signer};
use unc_o11y::testonly::init_test_logger;
use unc_primitives::errors::StorageError;
use unc_primitives::shard_layout::{ShardLayout, ShardUId};
use unc_primitives::transaction::SignedTransaction;
use unc_primitives::trie_key::TrieKey;
use unc_primitives::types::AccountId;
use unc_primitives_core::types::BlockHeight;
use unc_store::flat::{
    store_helper, FetchingStateStatus, FlatStorageCreationStatus, FlatStorageManager,
    FlatStorageReadyStatus, FlatStorageStatus, NUM_PARTS_IN_ONE_STEP,
};
use unc_store::test_utils::create_test_store;
use unc_store::{KeyLookupMode, Store, TrieTraversalItem};
use unc_vm_runner::logic::TrieNodesCount;

use crate::tests::client::process_blocks::deploy_test_contract_with_protocol_version;
use unc_parameters::ExtCosts;
use unc_primitives::test_utils::encode;
use unc_primitives::transaction::{Action, ExecutionMetadata, FunctionCallAction, Transaction};
use unc_primitives::version::ProtocolFeature;
use unc_primitives_core::hash::CryptoHash;
use unc_primitives_core::types::Gas;

/// Height on which we start flat storage background creation.
const START_HEIGHT: BlockHeight = 7;

/// Number of steps which should be enough to create flat storage.
const CREATION_TIMEOUT: BlockHeight = 30;

/// Setup environment with one unc client for testing.
fn setup_env(genesis: &Genesis, store: Store) -> TestEnv {
    let chain_genesis = ChainGenesis::new(genesis);
    TestEnv::builder(chain_genesis)
        .stores(vec![store])
        .real_epoch_managers(&genesis.config)
        .nightshade_runtimes(genesis)
        .build()
}

/// Waits for flat storage creation on given shard for `CREATION_TIMEOUT` blocks.
/// We have a pause after processing each block because state data is being fetched in rayon threads,
/// but we expect it to finish in <30s because state is small and there is only one state part.
/// Returns next block height available to produce.
fn wait_for_flat_storage_creation(
    env: &mut TestEnv,
    start_height: BlockHeight,
    shard_uid: ShardUId,
    produce_blocks: bool,
) -> BlockHeight {
    let store = env.clients[0].runtime_adapter.store().clone();
    let mut next_height = start_height;
    let mut prev_status = store_helper::get_flat_storage_status(&store, shard_uid).unwrap();
    while next_height < start_height + CREATION_TIMEOUT {
        if produce_blocks {
            env.produce_block(0, next_height);
        }
        env.clients[0].run_flat_storage_creation_step().unwrap();

        let status = store_helper::get_flat_storage_status(&store, shard_uid).unwrap();
        // Check validity of state transition for flat storage creation.
        match &prev_status {
            FlatStorageStatus::Empty => assert_matches!(
                status,
                FlatStorageStatus::Creation(FlatStorageCreationStatus::SavingDeltas)
            ),
            FlatStorageStatus::Creation(FlatStorageCreationStatus::SavingDeltas) => {
                assert_matches!(
                    status,
                    FlatStorageStatus::Creation(FlatStorageCreationStatus::SavingDeltas)
                        | FlatStorageStatus::Creation(FlatStorageCreationStatus::FetchingState(_))
                )
            }
            FlatStorageStatus::Creation(FlatStorageCreationStatus::FetchingState(_)) => {
                assert_matches!(
                    status,
                    FlatStorageStatus::Creation(FlatStorageCreationStatus::FetchingState(_))
                        | FlatStorageStatus::Creation(FlatStorageCreationStatus::CatchingUp(_))
                )
            }
            FlatStorageStatus::Creation(FlatStorageCreationStatus::CatchingUp(_)) => {
                assert_matches!(
                    status,
                    FlatStorageStatus::Creation(FlatStorageCreationStatus::CatchingUp(_))
                        | FlatStorageStatus::Ready(_)
                )
            }
            _ => {
                panic!("Invalid status {prev_status:?} observed during flat storage creation for height {next_height}");
            }
        }
        tracing::info!("Flat Creation status: {:?}", status);

        prev_status = status;
        next_height += 1;
        if matches!(prev_status, FlatStorageStatus::Ready(_)) {
            break;
        }

        thread::sleep(Duration::from_secs(1));
    }
    let flat_storage_manager = get_flat_storage_manager(&env);
    let status = flat_storage_manager.get_flat_storage_status(shard_uid);
    assert_matches!(
        status,
        FlatStorageStatus::Ready(_),
        "Client couldn't create flat storage until block {next_height}, status: {status:?}"
    );
    assert!(flat_storage_manager.get_flat_storage_for_shard(shard_uid).is_some());

    // We don't expect any forks in the chain after flat storage head, so the number of
    // deltas stored on DB should be exactly 2, as there are only 2 blocks after
    // the final block.
    let deltas_in_metadata =
        store_helper::get_all_deltas_metadata(&store, shard_uid).unwrap().len() as u64;
    assert_eq!(deltas_in_metadata, 2);

    next_height
}

/// Check correctness of flat storage creation.
#[test]
fn test_flat_storage_creation_sanity() {
    init_test_logger();
    let genesis = Genesis::test(vec!["test0".parse().unwrap()], 1);
    let shard_uid = genesis.config.shard_layout.shard_uids().next().unwrap();
    let store = create_test_store();

    // Process some blocks with flat storage. Then remove flat storage data from disk.
    {
        let mut env = setup_env(&genesis, store.clone());
        let signer = InMemorySigner::from_seed("test0".parse().unwrap(), KeyType::ED25519, "test0");
        let genesis_hash = *env.clients[0].chain.genesis().hash();
        for height in 1..START_HEIGHT {
            env.produce_block(0, height);

            let tx = SignedTransaction::send_money(
                height,
                "test0".parse().unwrap(),
                "test0".parse().unwrap(),
                &signer,
                1,
                genesis_hash,
            );
            assert_eq!(env.clients[0].process_tx(tx, false, false), ProcessTxResponse::ValidTx);
        }

        // If chain was initialized from scratch, flat storage state should be created. During block processing, flat
        // storage head should be moved to block `START_HEIGHT - 4`.
        let flat_head_height = START_HEIGHT - 4;
        let expected_flat_storage_head =
            env.clients[0].chain.get_block_hash_by_height(flat_head_height).unwrap();
        let status = store_helper::get_flat_storage_status(&store, shard_uid);
        if let Ok(FlatStorageStatus::Ready(FlatStorageReadyStatus { flat_head })) = status {
            assert_eq!(flat_head.hash, expected_flat_storage_head);
            assert_eq!(flat_head.height, flat_head_height);
        } else {
            panic!("expected FlatStorageStatus::Ready status, got {status:?}");
        }

        // Deltas for blocks until `flat_head_height` should not exist.
        for height in 0..=flat_head_height {
            let block_hash = env.clients[0].chain.get_block_hash_by_height(height).unwrap();
            assert_eq!(store_helper::get_delta_changes(&store, shard_uid, block_hash), Ok(None));
        }
        // Deltas for blocks until `START_HEIGHT` should still exist,
        // because they come after flat storage head.
        for height in flat_head_height + 1..START_HEIGHT {
            let block_hash = env.clients[0].chain.get_block_hash_by_height(height).unwrap();
            assert_matches!(
                store_helper::get_delta_changes(&store, shard_uid, block_hash),
                Ok(Some(_)),
                "height: {height}"
            );
        }

        let mut store_update = store.store_update();
        get_flat_storage_manager(&env)
            .remove_flat_storage_for_shard(shard_uid, &mut store_update)
            .unwrap();
        store_update.commit().unwrap();
    }

    // Create new chain and runtime using the same store. It should produce next blocks normally, but now it should
    // think that flat storage does not exist and background creation should be initiated.
    let mut env = setup_env(&genesis, store.clone());
    for height in START_HEIGHT..START_HEIGHT + 2 {
        env.produce_block(0, height);
    }
    assert!(get_flat_storage_manager(&env).get_flat_storage_for_shard(shard_uid).is_none());

    assert_eq!(
        store_helper::get_flat_storage_status(&store, shard_uid),
        Ok(FlatStorageStatus::Empty)
    );
    assert!(!env.clients[0].run_flat_storage_creation_step().unwrap());
    // At first, flat storage state should start saving deltas. Deltas for all newly processed blocks should be saved to
    // disk.
    assert_eq!(
        store_helper::get_flat_storage_status(&store, shard_uid),
        Ok(FlatStorageStatus::Creation(FlatStorageCreationStatus::SavingDeltas))
    );
    // Introduce fork block to check that deltas for it will be GC-d later.
    let fork_block = env.clients[0].produce_block(START_HEIGHT + 2).unwrap().unwrap();
    let fork_block_hash = *fork_block.hash();
    let next_block = env.clients[0].produce_block(START_HEIGHT + 3).unwrap().unwrap();
    let next_block_hash = *next_block.hash();
    env.process_block(0, fork_block, Provenance::PRODUCED);
    env.process_block(0, next_block, Provenance::PRODUCED);

    assert_matches!(
        store_helper::get_delta_changes(&store, shard_uid, fork_block_hash),
        Ok(Some(_))
    );
    assert_matches!(
        store_helper::get_delta_changes(&store, shard_uid, next_block_hash),
        Ok(Some(_))
    );

    // Produce new block and run flat storage creation step.
    // We started the node from height `START_HEIGHT - 1`, and now final head should move to height `START_HEIGHT`.
    // Because final head height became greater than height on which node started,
    // we must start fetching the state.
    env.produce_block(0, START_HEIGHT + 4);
    assert!(!env.clients[0].run_flat_storage_creation_step().unwrap());
    let final_block_hash = env.clients[0].chain.get_block_hash_by_height(START_HEIGHT).unwrap();
    assert_eq!(
        store_helper::get_flat_storage_status(&store, shard_uid),
        Ok(FlatStorageStatus::Creation(FlatStorageCreationStatus::FetchingState(
            FetchingStateStatus {
                block_hash: final_block_hash,
                part_id: 0,
                num_parts_in_step: NUM_PARTS_IN_ONE_STEP,
                num_parts: 1,
            }
        )))
    );

    wait_for_flat_storage_creation(&mut env, START_HEIGHT + 5, shard_uid, true);
}

/// Check that client can create flat storage on some shard while it already exists on another shard.
#[test]
fn test_flat_storage_creation_two_shards() {
    init_test_logger();
    let num_shards = 2;
    let genesis =
        Genesis::test_sharded_new_version(vec!["test0".parse().unwrap()], 1, vec![1; num_shards]);
    let shard_uids: Vec<_> = genesis.config.shard_layout.shard_uids().collect();
    let store = create_test_store();

    // Process some blocks with flat storages for two shards. Then remove flat storage data from disk for shard 0.
    {
        let mut env = setup_env(&genesis, store.clone());
        let signer = InMemorySigner::from_seed("test0".parse().unwrap(), KeyType::ED25519, "test0");
        let genesis_hash = *env.clients[0].chain.genesis().hash();
        for height in 1..START_HEIGHT {
            env.produce_block(0, height);

            let tx = SignedTransaction::send_money(
                height,
                "test0".parse().unwrap(),
                "test0".parse().unwrap(),
                &signer,
                1,
                genesis_hash,
            );
            assert_eq!(env.clients[0].process_tx(tx, false, false), ProcessTxResponse::ValidTx);
        }

        for &shard_uid in shard_uids.iter() {
            assert_matches!(
                store_helper::get_flat_storage_status(&store, shard_uid),
                Ok(FlatStorageStatus::Ready(_))
            );
        }

        let mut store_update = store.store_update();
        get_flat_storage_manager(&env)
            .remove_flat_storage_for_shard(shard_uids[0], &mut store_update)
            .unwrap();
        store_update.commit().unwrap();
    }

    // Check that flat storage is not ready for shard 0 but ready for shard 1.
    let mut env = setup_env(&genesis, store.clone());
    assert!(get_flat_storage_manager(&env).get_flat_storage_for_shard(shard_uids[0]).is_none());
    assert_matches!(
        store_helper::get_flat_storage_status(&store, shard_uids[0]),
        Ok(FlatStorageStatus::Empty)
    );
    assert!(get_flat_storage_manager(&env).get_flat_storage_for_shard(shard_uids[1]).is_some());
    assert_matches!(
        store_helper::get_flat_storage_status(&store, shard_uids[1]),
        Ok(FlatStorageStatus::Ready(_))
    );

    wait_for_flat_storage_creation(&mut env, START_HEIGHT, shard_uids[0], true);
}

/// Check that flat storage creation can be started from intermediate state where one
/// of state parts is already fetched.
#[test]
fn test_flat_storage_creation_start_from_state_part() {
    init_test_logger();
    // Create several accounts to ensure that state is non-trivial.
    let accounts =
        (0..4).map(|i| AccountId::from_str(&format!("test{}", i)).unwrap()).collect::<Vec<_>>();
    let genesis = Genesis::test(accounts, 1);
    let shard_uid = genesis.config.shard_layout.shard_uids().next().unwrap();
    let store = create_test_store();

    // Process some blocks with flat storage.
    // Reshard into two parts and return trie keys corresponding to each part.
    const NUM_PARTS: u64 = 2;
    let trie_keys: Vec<_> = {
        let mut env = setup_env(&genesis, store.clone());
        for height in 1..START_HEIGHT {
            env.produce_block(0, height);
        }

        assert_matches!(
            store_helper::get_flat_storage_status(&store, shard_uid),
            Ok(FlatStorageStatus::Ready(_))
        );

        let block_hash = env.clients[0].chain.get_block_hash_by_height(START_HEIGHT - 1).unwrap();
        let state_root =
            *env.clients[0].chain.get_chunk_extra(&block_hash, &shard_uid).unwrap().state_root();
        let trie = env.clients[0]
            .chain
            .runtime_adapter
            .get_trie_for_shard(0, &block_hash, state_root, true)
            .unwrap();
        (0..NUM_PARTS)
            .map(|part_id| {
                let path_begin = trie.find_state_part_boundary(part_id, NUM_PARTS).unwrap();
                let path_end = trie.find_state_part_boundary(part_id + 1, NUM_PARTS).unwrap();
                let mut trie_iter = trie.iter().unwrap();
                let mut keys = vec![];
                for item in trie_iter.visit_nodes_interval(&path_begin, &path_end).unwrap() {
                    if let TrieTraversalItem { key: Some(trie_key), .. } = item {
                        keys.push(trie_key);
                    }
                }
                keys
            })
            .collect()
    };
    assert!(!trie_keys[0].is_empty());
    assert!(!trie_keys[1].is_empty());

    {
        // Remove keys of part 1 from the flat state.
        // Manually set flat storage creation status to the step when it should start from fetching part 1.
        let status = store_helper::get_flat_storage_status(&store, shard_uid);
        let flat_head = if let Ok(FlatStorageStatus::Ready(ready_status)) = status {
            ready_status.flat_head.hash
        } else {
            panic!("expected FlatStorageStatus::Ready, got: {status:?}");
        };
        let mut store_update = store.store_update();
        for key in trie_keys[1].iter() {
            store_helper::set_flat_state_value(&mut store_update, shard_uid, key.clone(), None);
        }
        store_helper::set_flat_storage_status(
            &mut store_update,
            shard_uid,
            FlatStorageStatus::Creation(FlatStorageCreationStatus::FetchingState(
                FetchingStateStatus {
                    block_hash: flat_head,
                    part_id: 1,
                    num_parts_in_step: 1,
                    num_parts: NUM_PARTS,
                },
            )),
        );
        store_update.commit().unwrap();

        // Re-create runtime, check that flat storage is not created yet.
        let mut env = setup_env(&genesis, store);
        assert!(get_flat_storage_manager(&env).get_flat_storage_for_shard(shard_uid).is_none());

        // Run chain for a couple of blocks and check that flat storage for shard 0 is eventually created.
        let next_height = wait_for_flat_storage_creation(&mut env, START_HEIGHT, shard_uid, true);

        // Check that all the keys are present in flat storage.
        let block_hash = env.clients[0].chain.get_block_hash_by_height(next_height - 1).unwrap();
        let state_root =
            *env.clients[0].chain.get_chunk_extra(&block_hash, &shard_uid).unwrap().state_root();
        let trie = env.clients[0]
            .chain
            .runtime_adapter
            .get_trie_for_shard(0, &block_hash, state_root, true)
            .unwrap();
        for part_trie_keys in trie_keys.iter() {
            for trie_key in part_trie_keys.iter() {
                assert_matches!(
                    trie.get_optimized_ref(&trie_key, KeyLookupMode::FlatStorage),
                    Ok(Some(_))
                );
            }
        }
        assert_eq!(trie.get_trie_nodes_count(), TrieNodesCount { db_reads: 0, mem_reads: 0 });
    }
}

/// Tests the scenario where we start flat storage migration, and get just a few new blocks.
/// (in this test we still generate 3 blocks in order to generate deltas).
#[test]
fn test_catchup_succeeds_even_if_no_new_blocks() {
    init_test_logger();
    let genesis = Genesis::test(vec!["test0".parse().unwrap()], 1);
    let store = create_test_store();
    let shard_uid = ShardLayout::v0_single_shard().shard_uids().next().unwrap();

    // Process some blocks with flat storage. Then remove flat storage data from disk.
    {
        let mut env = setup_env(&genesis, store.clone());
        for height in 1..START_HEIGHT {
            env.produce_block(0, height);
        }
        // Remove flat storage.
        let mut store_update = store.store_update();
        get_flat_storage_manager(&env)
            .remove_flat_storage_for_shard(shard_uid, &mut store_update)
            .unwrap();
        store_update.commit().unwrap();
    }
    let mut env = setup_env(&genesis, store.clone());
    assert!(get_flat_storage_manager(&env).get_flat_storage_for_shard(shard_uid).is_none());
    assert_eq!(
        store_helper::get_flat_storage_status(&store, shard_uid),
        Ok(FlatStorageStatus::Empty)
    );
    // Create 3 more blocks (so that the deltas are generated) - and assume that no new blocks are received.
    // In the future, we should also support the scenario where no new blocks are created.

    for block_height in START_HEIGHT + 1..=START_HEIGHT + 3 {
        env.produce_block(0, block_height);
    }

    assert!(!env.clients[0].run_flat_storage_creation_step().unwrap());
    wait_for_flat_storage_creation(&mut env, START_HEIGHT + 3, shard_uid, false);
}

/// Tests the flat storage iterator. Running on a chain with 3 shards, and couple blocks produced.
#[test]
fn test_flat_storage_iter() {
    init_test_logger();
    let num_shards = 3;
    let shard_layout =
        ShardLayout::v1(vec!["test0".parse().unwrap(), "test1".parse().unwrap()], None, 0);

    let genesis = Genesis::test_with_seeds(
        vec!["test0".parse().unwrap()],
        1,
        vec![1; num_shards],
        shard_layout.clone(),
    );

    let store = create_test_store();

    let mut env = setup_env(&genesis, store.clone());
    for height in 1..START_HEIGHT {
        env.produce_block(0, height);
    }

    for shard_id in 0..3 {
        let shard_uid = ShardUId::from_shard_id_and_layout(shard_id, &shard_layout);
        let items: Vec<_> =
            store_helper::iter_flat_state_entries(shard_uid, &store, None, None).collect();

        match shard_id {
            0 => {
                assert_eq!(2, items.len());
                // Two entries - one for 'unc' system account, the other for the contract.
                assert_eq!(
                    TrieKey::Account { account_id: "unc".parse().unwrap() }.to_vec(),
                    items[0].as_ref().unwrap().0.to_vec()
                );
            }
            1 => {
                // Two entries - one for account, the other for contract.
                assert_eq!(2, items.len());
                assert_eq!(
                    TrieKey::Account { account_id: "test0".parse().unwrap() }.to_vec(),
                    items[0].as_ref().unwrap().0.to_vec()
                );
            }
            2 => {
                // Test1 account was not created yet - so no entries.
                assert_eq!(0, items.len());
            }
            _ => {
                panic!("Unexpected shard_id");
            }
        }
    }
}

#[test]
/// Initializes flat storage, then creates a Trie to read the flat storage
/// exactly at the flat head block.
/// Add another block to the flat state, which moves flat head and makes the
/// state of the previous flat head inaccessible.
fn test_not_supported_block() {
    init_test_logger();
    let genesis = Genesis::test(vec!["test0".parse().unwrap()], 1);
    let shard_layout = ShardLayout::v0_single_shard();
    let shard_uid = shard_layout.shard_uids().next().unwrap();
    let store = create_test_store();

    let mut env = setup_env(&genesis, store);
    let signer = InMemorySigner::from_seed("test0".parse().unwrap(), KeyType::ED25519, "test0");
    let genesis_hash = *env.clients[0].chain.genesis().hash();

    // Produce blocks up to `START_HEIGHT`.
    for height in 1..START_HEIGHT {
        env.produce_block(0, height);
        let tx = SignedTransaction::send_money(
            height,
            "test0".parse().unwrap(),
            "test0".parse().unwrap(),
            &signer,
            1,
            genesis_hash,
        );
        assert_eq!(env.clients[0].process_tx(tx, false, false), ProcessTxResponse::ValidTx);
    }

    let flat_head_height = START_HEIGHT - 4;
    // Trie key which must exist in the storage.
    let trie_key_bytes =
        unc_primitives::trie_key::TrieKey::Account { account_id: "test0".parse().unwrap() }
            .to_vec();
    // Create trie, which includes creating chunk view, and get `ValueRef`s
    // for post state roots for blocks `START_HEIGHT - 3` and `START_HEIGHT - 2`.
    // After creating the first trie, produce block `START_HEIGHT` which moves flat storage
    // head 1 block further and invalidates it.
    let mut get_ref_results = vec![];
    for height in flat_head_height..START_HEIGHT - 1 {
        let block_hash = env.clients[0].chain.get_block_hash_by_height(height).unwrap();
        let state_root = *env.clients[0]
            .chain
            .get_chunk_extra(&block_hash, &ShardUId::from_shard_id_and_layout(0, &shard_layout))
            .unwrap()
            .state_root();

        let trie = env.clients[0]
            .runtime_adapter
            .get_trie_for_shard(shard_uid.shard_id(), &block_hash, state_root, true)
            .unwrap();
        if height == flat_head_height {
            env.produce_block(0, START_HEIGHT);
        }
        get_ref_results.push(trie.get_optimized_ref(&trie_key_bytes, KeyLookupMode::FlatStorage));
    }

    // The first result should be FlatStorageError, because we can't read from first chunk view anymore.
    // But the node must not panic as this is normal behaviour.
    // Ideally it should be tested on chain level, but there is no easy way to
    // postpone applying chunks reliably.
    assert_matches!(get_ref_results[0], Err(StorageError::FlatStorageBlockNotSupported(_)));
    // For the second result chunk view is valid, so result is Ok.
    assert_matches!(get_ref_results[1], Ok(Some(_)));
}


/// Check that after flat storage upgrade:
/// - value read from contract is the same;
/// - touching trie node cost for read decreases to zero.
#[test]
fn test_flat_storage_upgrade() {
    // The immediate protocol upgrade needs to be set for this test to pass in
    // the release branch where the protocol upgrade date is set.
    std::env::set_var("unc_TESTS_IMMEDIATE_PROTOCOL_UPGRADE", "1");

    let mut genesis = Genesis::test(vec!["test0".parse().unwrap(), "test1".parse().unwrap()], 1);
    let epoch_length = 12;
    let new_protocol_version = ProtocolFeature::FlatStorageReads.protocol_version();
    let old_protocol_version = new_protocol_version - 1;
    genesis.config.epoch_length = epoch_length;
    genesis.config.protocol_version = old_protocol_version;
    let chain_genesis = ChainGenesis::new(&genesis);
    let runtime_config = unc_parameters::RuntimeConfigStore::new(None);
    let mut env = TestEnv::builder(chain_genesis)
        .real_epoch_managers(&genesis.config)
        .nightshade_runtimes_with_runtime_config_store(&genesis, vec![runtime_config])
        .build();

    // We assume that it is enough to process 4 blocks to get a single txn included and processed.
    // At the same time, once we process `>= 2 * epoch_length` blocks, protocol can get
    // auto-upgraded to latest version. We use this value to process 3 transactions for older
    // protocol version. So we choose this value to be `epoch_length / 3` and we process only
    // `epoch_length` blocks in total.
    // TODO (#8703): resolve this properly
    let blocks_to_process_txn = epoch_length / 3;

    // Deploy contract to state.
    deploy_test_contract_with_protocol_version(
        &mut env,
        "test0".parse().unwrap(),
        unc_test_contracts::backwards_compatible_rs_contract(),
        blocks_to_process_txn,
        1,
        old_protocol_version,
    );

    let signer = InMemorySigner::from_seed("test0".parse().unwrap(), KeyType::ED25519, "test0");
    let gas = 20_000_000_000_000;
    let tx = Transaction {
        signer_id: "test0".parse().unwrap(),
        receiver_id: "test0".parse().unwrap(),
        public_key: signer.public_key(),
        actions: vec![],
        nonce: 0,
        block_hash: CryptoHash::default(),
    };

    // Write key-value pair to state.
    {
        let write_value_action = vec![Action::FunctionCall(Box::new(FunctionCallAction {
            args: encode(&[1u64, 10u64]),
            method_name: "write_key_value".to_string(),
            gas,
            deposit: 0,
        }))];
        let tip = env.clients[0].chain.head().unwrap();
        let signed_transaction = Transaction {
            nonce: 10,
            block_hash: tip.last_block_hash,
            actions: write_value_action,
            ..tx.clone()
        }
        .sign(&signer);
        let tx_hash = signed_transaction.get_hash();
        assert_eq!(
            env.clients[0].process_tx(signed_transaction, false, false),
            ProcessTxResponse::ValidTx
        );
        for i in 0..blocks_to_process_txn {
            env.produce_block(0, tip.height + i + 1);
        }

        env.clients[0].chain.get_final_transaction_result(&tx_hash).unwrap().assert_success();
    }

    let touching_trie_node_costs: Vec<_> = (0..2)
        .map(|i| {
            let read_value_action = vec![Action::FunctionCall(Box::new(FunctionCallAction {
                args: encode(&[1u64]),
                method_name: "read_value".to_string(),
                gas,
                deposit: 0,
            }))];
            let tip = env.clients[0].chain.head().unwrap();
            let signed_transaction = Transaction {
                nonce: 20 + i,
                block_hash: tip.last_block_hash,
                actions: read_value_action,
                ..tx.clone()
            }
            .sign(&signer);
            let tx_hash = signed_transaction.get_hash();
            assert_eq!(
                env.clients[0].process_tx(signed_transaction, false, false),
                ProcessTxResponse::ValidTx
            );
            for i in 0..blocks_to_process_txn {
                env.produce_block(0, tip.height + i + 1);
            }
            if i == 0 {
                env.upgrade_protocol(new_protocol_version);
            }

            let final_transaction_result =
                env.clients[0].chain.get_final_transaction_result(&tx_hash).unwrap();
            final_transaction_result.assert_success();
            let receipt_id = final_transaction_result.receipts_outcome[0].id;
            let metadata = env.clients[0]
                .chain
                .get_execution_outcome(&receipt_id)
                .unwrap()
                .outcome_with_id
                .outcome
                .metadata;
            if let ExecutionMetadata::V3(profile_data) = metadata {
                profile_data.get_ext_cost(ExtCosts::touching_trie_node)
            } else {
                panic!("Too old version of metadata: {metadata:?}");
            }
        })
        .collect();

    // Guaranteed touching trie node cost in all protocol versions until
    // `ProtocolFeature::FlatStorageReads`, included.
    let touching_trie_node_base_cost: Gas = 16_101_955_926;

    // For the first read, cost should be 3 TTNs because trie path is:
    // (Branch) -> (Extension) -> (Leaf) -> (Value)
    // but due to a bug in storage_read we don't charge for Value.
    assert_eq!(touching_trie_node_costs[0], touching_trie_node_base_cost * 3);

    // For the second read, we don't go to Flat storage and don't charge TTN.
    assert_eq!(touching_trie_node_costs[1], 0);
}


fn get_flat_storage_manager(env: &TestEnv) -> FlatStorageManager {
    env.clients[0].chain.runtime_adapter.get_flat_storage_manager()
}
