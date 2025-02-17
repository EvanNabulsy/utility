use once_cell::sync::Lazy;
use unc_o11y::metrics::{try_create_int_counter, try_create_int_gauge, IntCounter, IntGauge};

pub static CONFIG_RELOADS: Lazy<IntCounter> = Lazy::new(|| {
    try_create_int_counter(
        "unc_config_reloads_total",
        "Number of times the configs were reloaded during the current run of the process",
    )
    .unwrap()
});

pub static CONFIG_RELOAD_TIMESTAMP: Lazy<IntGauge> = Lazy::new(|| {
    try_create_int_gauge(
        "unc_config_reload_timestamp_seconds",
        "Timestamp of the last reload of the config",
    )
    .unwrap()
});
