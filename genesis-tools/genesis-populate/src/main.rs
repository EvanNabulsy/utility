use clap::{Arg, Command};
use framework::{get_default_home, load_config};
use genesis_populate::GenesisBuilder;
use std::path::PathBuf;
use unc_chain_configs::GenesisValidationMode;

fn main() {
    let matches = Command::new("Genesis populator")
        .arg(
            Arg::new("home")
                .long("home")
                .default_value(get_default_home().into_os_string())
                .value_parser(clap::value_parser!(PathBuf))
                .help("Directory for config and data (default \"~/.unc\")")
                .action(clap::ArgAction::Set),
        )
        .arg(
            Arg::new("additional-accounts-num")
                .long("additional-accounts-num")
                .required(true)
                .action(clap::ArgAction::Set)
                .help(
                    "Number of additional accounts per shard to add directly to the trie \
                     (TESTING ONLY)",
                ),
        )
        .get_matches();

    let home_dir = matches.get_one::<PathBuf>("home").unwrap();
    let additional_accounts_num = matches
        .get_one::<String>("additional-accounts-num")
        .map(|x| x.parse::<u64>().expect("Failed to parse number of additional accounts."))
        .unwrap();
    let unc_config = load_config(home_dir, GenesisValidationMode::Full)
        .unwrap_or_else(|e| panic!("Error loading config: {:#}", e));

    let store = unc_store::NodeStorage::opener(
        home_dir,
        unc_config.config.archive,
        &unc_config.config.store,
        None,
    )
    .open()
    .unwrap()
    .get_hot_store();
    GenesisBuilder::from_config_and_store(home_dir, unc_config, store)
        .add_additional_accounts(additional_accounts_num)
        .add_additional_accounts_contract(unc_test_contracts::trivial_contract().to_vec())
        .print_progress()
        .build()
        .unwrap()
        .dump_state()
        .unwrap();
}
