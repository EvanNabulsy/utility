#![cfg_attr(not(feature = "nightly"), allow(unused_imports))]

use crate::tests::client::process_blocks::deploy_test_contract;
use framework::config::GenesisExt;
use framework::test_utils::TestEnvNightshadeSetupExt;
use unc_chain::ChainGenesis;
use unc_chain_configs::Genesis;
use unc_client::test_utils::TestEnv;
use unc_client::ProcessTxResponse;
use unc_crypto::{InMemorySigner, KeyType, Signer};
use unc_parameters::RuntimeConfigStore;
use unc_primitives::hash::CryptoHash;
use unc_primitives::transaction::{Action, FunctionCallAction, Transaction};

#[cfg_attr(all(target_arch = "aarch64", target_vendor = "apple"), ignore)]
#[test]
fn test_uncvm_upgrade() {
    let mut capture = unc_o11y::testonly::TracingCapture::enable();

    let old_protocol_version =
        unc_primitives::version::ProtocolFeature::NearVmRuntime.protocol_version() - 1;
    let new_protocol_version = old_protocol_version + 1;

    // Prepare TestEnv with a contract at the old protocol version.
    let mut env = {
        let epoch_length = 5;
        let mut genesis =
            Genesis::test(vec!["test0".parse().unwrap(), "test1".parse().unwrap()], 1);
        genesis.config.epoch_length = epoch_length;
        genesis.config.protocol_version = old_protocol_version;
        let chain_genesis = ChainGenesis::new(&genesis);
        let mut env = TestEnv::builder(chain_genesis)
            .real_epoch_managers(&genesis.config)
            .nightshade_runtimes_with_runtime_config_store(
                &genesis,
                vec![RuntimeConfigStore::new(None)],
            )
            .build();

        deploy_test_contract(
            &mut env,
            "test0".parse().unwrap(),
            unc_test_contracts::backwards_compatible_rs_contract(),
            epoch_length,
            1,
        );
        env
    };

    let signer = InMemorySigner::from_seed("test0".parse().unwrap(), KeyType::ED25519, "test0");
    let tx = Transaction {
        signer_id: "test0".parse().unwrap(),
        receiver_id: "test0".parse().unwrap(),
        public_key: signer.public_key(),
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "log_something".to_string(),
            args: Vec::new(),
            gas: 100_000_000_000_000,
            deposit: 0,
        }))],

        nonce: 0,
        block_hash: CryptoHash::default(),
    };

    // Run the transaction & collect the logs.
    let logs_at_old_version = {
        let tip = env.clients[0].chain.head().unwrap();
        let signed_transaction =
            Transaction { nonce: 10, block_hash: tip.last_block_hash, ..tx.clone() }.sign(&signer);
        assert_eq!(
            env.clients[0].process_tx(signed_transaction, false, false),
            ProcessTxResponse::ValidTx
        );
        for i in 0..3 {
            env.produce_block(0, tip.height + i + 1);
        }
        capture.drain()
    };

    env.upgrade_protocol(new_protocol_version);

    // Re-run the transaction.
    let logs_at_new_version = {
        let tip = env.clients[0].chain.head().unwrap();
        let signed_transaction =
            Transaction { nonce: 11, block_hash: tip.last_block_hash, ..tx }.sign(&signer);
        assert_eq!(
            env.clients[0].process_tx(signed_transaction, false, false),
            ProcessTxResponse::ValidTx
        );
        for i in 0..3 {
            env.produce_block(0, tip.height + i + 1);
        }
        capture.drain()
    };

    assert!(logs_at_old_version.iter().any(|l| l.contains(&"vm_kind=Wasmer2")));
    assert!(dbg!(logs_at_new_version).iter().any(|l| l.contains(&"vm_kind=NearVm")));
}
