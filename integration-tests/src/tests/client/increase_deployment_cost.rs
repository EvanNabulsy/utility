use assert_matches::assert_matches;
use framework::config::GenesisExt;
use framework::test_utils::TestEnvNightshadeSetupExt;
use unc_chain::ChainGenesis;
use unc_chain_configs::Genesis;
use unc_client::test_utils::TestEnv;
use unc_crypto::{InMemorySigner, KeyType};
use unc_parameters::vm::VMKind;
use unc_parameters::RuntimeConfigStore;
use unc_primitives::transaction::{Action, DeployContractAction};
use unc_primitives::version::ProtocolFeature;
use unc_primitives::views::FinalExecutionStatus;
use unc_primitives_core::version::PROTOCOL_VERSION;

/// Tests if the cost of deployment is higher after the protocol update 53
#[test]
fn test_deploy_cost_increased() {
    // The immediate protocol upgrade needs to be set for this test to pass in
    // the release branch where the protocol upgrade date is set.
    std::env::set_var("unc_TESTS_IMMEDIATE_PROTOCOL_UPGRADE", "1");

    let new_protocol_version = ProtocolFeature::IncreaseDeploymentCost.protocol_version();
    let old_protocol_version = new_protocol_version - 1;

    let config_store = RuntimeConfigStore::new(None);
    let config = &config_store.get_config(PROTOCOL_VERSION).wasm_config;
    let contract_size = 1024 * 1024;
    let test_contract = unc_test_contracts::sized_contract(contract_size);
    // Run code through preparation for validation. (Deploying will succeed either way).
    unc_vm_runner::prepare::prepare_contract(&test_contract, config, VMKind::Wasmer2).unwrap();

    // Prepare TestEnv with a contract at the old protocol version.
    let epoch_length = 5;
    let mut env = {
        let mut genesis = Genesis::test(vec!["test0".parse().unwrap()], 1);
        genesis.config.epoch_length = epoch_length;
        genesis.config.protocol_version = old_protocol_version;
        let chain_genesis = ChainGenesis::new(&genesis);
        TestEnv::builder(chain_genesis)
            .real_epoch_managers(&genesis.config)
            .nightshade_runtimes_with_runtime_config_store(
                &genesis,
                vec![RuntimeConfigStore::new(None)],
            )
            .build()
    };

    let signer = InMemorySigner::from_seed("test0".parse().unwrap(), KeyType::ED25519, "test0");
    let actions = vec![Action::DeployContract(DeployContractAction { code: test_contract })];

    let tx = env.tx_from_actions(actions.clone(), &signer, signer.account_id.clone());
    let old_outcome = env.execute_tx(tx).unwrap();

    env.upgrade_protocol(new_protocol_version);

    let tx = env.tx_from_actions(actions, &signer, signer.account_id.clone());
    let new_outcome = env.execute_tx(tx).unwrap();

    assert_matches!(old_outcome.status, FinalExecutionStatus::SuccessValue(_));
    assert_matches!(new_outcome.status, FinalExecutionStatus::SuccessValue(_));

    let old_deploy_gas = old_outcome.receipts_outcome[0].outcome.gas_burnt;
    let new_deploy_gas = new_outcome.receipts_outcome[0].outcome.gas_burnt;
    assert!(new_deploy_gas > old_deploy_gas);
    assert_eq!(new_deploy_gas - old_deploy_gas, contract_size as u64 * (64_572_944 - 6_812_999));
}
